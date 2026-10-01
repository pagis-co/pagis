import type { ProgressInfo, UpdateCheckResult, UpdateDownloadedEvent, UpdateInfo } from 'electron-updater'

/** The time between two checks for an Update (ADR-0027). */
export const CHECK_EVERY_MS = 24 * 60 * 60 * 1000
/** How often the client compares the clock with the time of the last
 *  check. Node stops its timers while a Mac sleeps, so a 24-hour timer
 *  would wait for 24 hours of awake time. */
const TICK_MS = 60 * 60 * 1000

/** Where the Client App is with an Update. */
export type UpdateState =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'downloading'; version: string; percent: number }
  /** Squirrel.Mac holds the Update and accepted its code signature. */
  | { kind: 'ready'; version: string }
  | { kind: 'failed'; reason: string }

/** What "Check for Updates…" found. */
export type CheckResult =
  | { kind: 'up-to-date' }
  | { kind: 'found'; version: string }
  | { kind: 'ready'; version: string }
  | { kind: 'failed'; reason: string }

/** What the Client App uses of the `autoUpdater` of electron-updater. */
export interface Updater {
  autoDownload: boolean
  autoInstallOnAppQuit: boolean
  allowPrerelease: boolean
  checkForUpdates(): Promise<UpdateCheckResult | null>
  quitAndInstall(): void
  on(event: 'checking-for-update', listener: () => void): unknown
  on(event: 'update-not-available', listener: (info: UpdateInfo) => void): unknown
  on(event: 'update-available', listener: (info: UpdateInfo) => void): unknown
  on(event: 'download-progress', listener: (progress: ProgressInfo) => void): unknown
  on(event: 'update-downloaded', listener: (event: UpdateDownloadedEvent) => void): unknown
  on(event: 'error', listener: (error: Error) => void): unknown
}

/**
 * Squirrel.Mac, the `autoUpdater` of Electron. electron-updater gives it
 * each downloaded ZIP. It reports `update-downloaded` only when the new
 * bundle satisfies the designated requirement of the running one, and it
 * installs the Update when the app quits.
 */
export interface Installer {
  on(event: 'update-downloaded', listener: () => void): unknown
}

export interface UpdatesOptions {
  updater: Updater
  installer: Installer
  /** Each change of the state, for the menus. */
  onState(state: UpdateState): void
  /** Tell the Person that an Update is ready. */
  notify(version: string): void
}

/**
 * The Update of the Client App on macOS (ADR-0027). electron-updater
 * checks the GitHub releases of the app-update.yml in the package,
 * downloads an Update with no question, and Squirrel.Mac installs it at
 * quit or at "Restart to Update".
 *
 * A check starts only while no Update downloads or waits, as in VS Code:
 * the Person installs the ready Update, and the new Client App checks
 * again when it starts.
 */
export class Updates {
  private current: UpdateState = { kind: 'idle' }
  private downloaded: string | null = null
  private notified: string | null = null
  private timer: ReturnType<typeof setInterval> | null = null
  private lastCheck = 0

  constructor(private readonly options: UpdatesOptions) {
    const { updater, installer } = options
    updater.autoDownload = true
    updater.autoInstallOnAppQuit = true
    updater.allowPrerelease = false
    updater.on('checking-for-update', () => this.set({ kind: 'checking' }))
    updater.on('update-not-available', () => this.set({ kind: 'idle' }))
    updater.on('update-available', (info) => this.set({ kind: 'downloading', version: info.version, percent: 0 }))
    updater.on('download-progress', (progress) => {
      const state = this.current
      const percent = Math.floor(progress.percent)
      if (state.kind === 'downloading' && percent !== state.percent) this.set({ ...state, percent })
    })
    // electron-updater checked the SHA-512 of the feed. Squirrel.Mac now
    // checks the code signature, and the Update is ready after that.
    updater.on('update-downloaded', (event) => {
      this.downloaded = event.version
      this.set({ kind: 'downloading', version: event.version, percent: 100 })
    })
    installer.on('update-downloaded', () => this.ready())
    updater.on('error', (error) => this.set({ kind: 'failed', reason: reasonOf(error) }))
  }

  get state(): UpdateState {
    return this.current
  }

  /** Check now, then every 24 hours. A second start does nothing. */
  start(): void {
    if (this.timer !== null) return
    void this.check()
    this.timer = setInterval(() => {
      if (Date.now() - this.lastCheck >= CHECK_EVERY_MS) void this.check()
    }, TICK_MS)
  }

  stop(): void {
    if (this.timer !== null) clearInterval(this.timer)
    this.timer = null
  }

  /** Check for an Update. electron-updater downloads one that it finds. */
  async check(): Promise<CheckResult> {
    const state = this.current
    if (state.kind === 'downloading') return { kind: 'found', version: state.version }
    if (state.kind === 'ready') return state
    this.lastCheck = Date.now()
    try {
      const result = await this.options.updater.checkForUpdates()
      if (!result?.isUpdateAvailable) return { kind: 'up-to-date' }
      // A failed download goes to the `error` listener.
      void result.downloadPromise?.catch(() => undefined)
      return { kind: 'found', version: result.updateInfo.version }
    } catch (error) {
      const reason = reasonOf(error)
      this.set({ kind: 'failed', reason })
      return { kind: 'failed', reason }
    }
  }

  /** Quit, install the ready Update, and start the new Client App. */
  install(): void {
    if (this.current.kind !== 'ready') throw new Error('no Update is ready to install')
    this.options.updater.quitAndInstall()
  }

  private ready(): void {
    const version = this.downloaded
    if (version === null) return
    this.set({ kind: 'ready', version })
    if (this.notified === version) return
    this.notified = version
    this.options.notify(version)
  }

  private set(state: UpdateState): void {
    this.current = state
    this.options.onState(state)
  }
}

/** The first line of an error. An HTTP error of electron-updater adds
 *  the response headers on the lines after it. */
function reasonOf(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error)
  return message.split('\n')[0]
}
