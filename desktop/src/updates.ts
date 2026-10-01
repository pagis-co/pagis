import type { AppUpdater, ProgressInfo, UpdateCheckResult, UpdateDownloadedEvent, UpdateInfo } from 'electron-updater'

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
  /** The Update passed its check: Squirrel.Mac accepted its code
   *  signature, or it is on the signed checksum list of its release. The
   *  Client App prepares the restart (`UpdatesOptions.prepare`). */
  | { kind: 'preparing'; version: string }
  /** The Update passed its check, and the restart is prepared. */
  | { kind: 'ready'; version: string }
  | { kind: 'failed'; reason: string }

/** What "Check for Updates…" found. */
export type CheckResult =
  | { kind: 'up-to-date' }
  /** A connected Client App whose server runs no newer release. */
  | { kind: 'up-to-date-with-server'; server: string }
  | { kind: 'found'; version: string }
  | { kind: 'ready'; version: string }
  | { kind: 'failed'; reason: string }

/** The releases that electron-updater reads: a provider and its options. */
export type Feed = Parameters<AppUpdater['setFeedURL']>[0]

/** What a check reads (ADR-0027). */
export type UpdateSource =
  | { kind: 'feed'; feed: Feed }
  /** A connected Client App whose server runs no newer release than the
   *  client. There is no Update, and electron-updater reads nothing. */
  | { kind: 'server-not-newer'; server: string }

/** What the Client App uses of the `autoUpdater` of electron-updater. */
export interface Updater {
  autoDownload: boolean
  autoInstallOnAppQuit: boolean
  allowPrerelease: boolean
  setFeedURL(feed: Feed): void
  checkForUpdates(): Promise<UpdateCheckResult | null>
  quitAndInstall(): void
  /** Install the downloaded Update. Only the Linux updaters have it. */
  install?(isSilent: boolean, isForceRunAfter: boolean): boolean
  on(event: 'update-not-available', listener: (info: UpdateInfo) => void): unknown
  on(event: 'update-available', listener: (info: UpdateInfo) => void): unknown
  on(event: 'download-progress', listener: (progress: ProgressInfo) => void): unknown
  on(event: 'update-downloaded', listener: (event: UpdateDownloadedEvent) => void): unknown
  /** The AppImage updater gave the AppImage file the name of the new
   *  release. */
  on(event: 'appimage-filename-updated', listener: (file: string) => void): unknown
  on(event: 'error', listener: (error: Error) => void): unknown
}

/** The check of a Linux download against the signed checksum list of its
 *  release. It rejects with the reason when the download does not pass. */
type Verify = (update: UpdateDownloadedEvent) => Promise<void>

/**
 * What checks a downloaded Update and installs it (ADR-0027).
 *
 * - `squirrel`: Squirrel.Mac, the `autoUpdater` of Electron. electron-updater
 *   gives it each downloaded ZIP while `autoInstallOnAppQuit` is on. It
 *   reports `update-downloaded` only when the new bundle satisfies the
 *   designated requirement of the running one, and it installs the Update
 *   when the app quits.
 * - `appimage`: electron-updater replaces the AppImage `file`, at "Restart
 *   to Update" and at quit. `restart` quits and starts the new file after
 *   this process ended.
 * - `deb`: electron-updater installs the deb with `pkexec dpkg -i` only at
 *   "Restart to Update", because a password prompt at quit or at logout
 *   stops the shutdown.
 */
export type Installer =
  | { kind: 'squirrel'; squirrel: { on(event: 'update-downloaded', listener: () => void): unknown } }
  | { kind: 'appimage'; verify: Verify; file: string; restart(file: string): void }
  | { kind: 'deb'; verify: Verify }

export interface UpdatesOptions {
  updater: Updater
  installer: Installer
  /** Each change of the state, for the menus. */
  onState(state: UpdateState): void
  /** Tell the Person that an Update is ready. */
  notify(version: string): void
  /** Prepare the restart to the Update, before it is ready (ADR-0027). A
   *  failure does not stop the Update. */
  prepare(version: string): Promise<void>
  /** What the next check reads. A rejection fails the check with its
   *  reason. */
  source(): Promise<UpdateSource>
}

/**
 * The Update of the Client App (ADR-0027). Each check reads the feed that
 * the source gives, and electron-updater downloads an Update with no
 * question. The installer checks the download, and the Update is ready
 * after that check.
 *
 * A check starts only while no Update downloads or waits, as in VS Code:
 * the Person installs the ready Update, and the new Client App checks
 * again when it starts.
 */
export class Updates {
  private current: UpdateState = { kind: 'idle' }
  private downloaded: string | null = null
  private notified: string | null = null
  private installing = false
  // The AppImage file. An install can give it the name of the new release.
  private appImage: string | null = null
  private timer: ReturnType<typeof setInterval> | null = null
  private lastCheck = 0

  constructor(private readonly options: UpdatesOptions) {
    const { updater, installer } = options
    updater.autoDownload = true
    // electron-updater gives a download to Squirrel.Mac only while this is
    // on. On Linux it would install each download at quit, also one that
    // did not pass the check, so the Client App installs at quit itself
    // (`installAtQuit`).
    updater.autoInstallOnAppQuit = installer.kind === 'squirrel'
    updater.allowPrerelease = false
    updater.on('update-not-available', () => this.set({ kind: 'idle' }))
    updater.on('update-available', (info) => this.set({ kind: 'downloading', version: info.version, percent: 0 }))
    updater.on('download-progress', (progress) => {
      const state = this.current
      const percent = Math.floor(progress.percent)
      if (state.kind === 'downloading' && percent !== state.percent) this.set({ ...state, percent })
    })
    // electron-updater checked the SHA-512 of the feed. The installer now
    // checks the download, and the Update is ready after that.
    updater.on('update-downloaded', (event) => {
      this.set({ kind: 'downloading', version: event.version, percent: 100 })
      if (installer.kind === 'squirrel') this.downloaded = event.version
      else void this.checkDownload(installer.verify, event)
    })
    if (installer.kind === 'squirrel') {
      installer.squirrel.on('update-downloaded', () => {
        if (this.downloaded !== null) void this.ready(this.downloaded)
      })
    }
    if (installer.kind === 'appimage') {
      this.appImage = installer.file
      updater.on('appimage-filename-updated', (file) => { this.appImage = file })
    }
    updater.on('error', (error) => this.set({ kind: 'failed', reason: reasonOf(error) }))
  }

  get state(): UpdateState {
    return this.current
  }

  /** Whether the schedule of checks runs. */
  get running(): boolean {
    return this.timer !== null
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
    if (state.kind === 'downloading' || state.kind === 'preparing') return { kind: 'found', version: state.version }
    if (state.kind === 'ready') return state
    this.lastCheck = Date.now()
    // The source can ask a server before electron-updater reads a feed.
    this.set({ kind: 'checking' })
    try {
      const source = await this.options.source()
      if (source.kind === 'server-not-newer') {
        this.set({ kind: 'idle' })
        return { kind: 'up-to-date-with-server', server: source.server }
      }
      this.options.updater.setFeedURL(source.feed)
      const result = await this.options.updater.checkForUpdates()
      // An updater that is not active answers null and emits nothing.
      if (result === null) this.set({ kind: 'idle' })
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

  /**
   * Quit, install the ready Update, and start the new Client App.
   *
   * electron-updater starts a new AppImage before this process ends. The
   * new process then finds the single-instance lock taken and exits, and
   * Pagis does not come back. So an AppImage installs with no start, and
   * `restart` starts the new file after this process ended, as Electron
   * does for a deb.
   */
  install(): void {
    if (this.current.kind !== 'ready') throw new Error('no Update is ready to install')
    this.installing = true
    const { installer, updater } = this.options
    if (installer.kind !== 'appimage') updater.quitAndInstall()
    else if (this.installAppImage()) installer.restart(this.appImage ?? installer.file)
  }

  /**
   * Install a ready AppImage Update while the Client App quits, with the
   * call that electron-updater makes at quit. Squirrel.Mac installs at quit
   * by itself, and a deb installs only at "Restart to Update".
   */
  installAtQuit(): void {
    if (this.options.installer.kind !== 'appimage' || this.current.kind !== 'ready' || this.installing) return
    this.installing = true
    this.installAppImage()
  }

  /** Replace the AppImage file and start nothing. electron-updater reports
   *  a failure with `error`, and then this answers false. */
  private installAppImage(): boolean {
    return this.options.updater.install?.(true, false) ?? false
  }

  /** Check a Linux download. A result that comes after another state,
   *  such as an error of electron-updater, changes nothing. */
  private async checkDownload(verify: Verify, update: UpdateDownloadedEvent): Promise<void> {
    const current = () => this.current.kind === 'downloading' && this.current.version === update.version
    try {
      await verify(update)
    } catch (error) {
      if (current()) this.set({ kind: 'failed', reason: reasonOf(error) })
      return
    }
    if (current()) await this.ready(update.version)
  }

  /** The Update passed its check. Prepare the restart, then offer it.
   *  Squirrel.Mac can report the same Update again. */
  private async ready(version: string): Promise<void> {
    const state = this.current
    if ((state.kind === 'preparing' || state.kind === 'ready') && state.version === version) return
    this.set({ kind: 'preparing', version })
    try {
      await this.options.prepare(version)
    } catch (error) {
      console.error(`pagis: the client did not prepare the restart to Pagis ${version}: ${reasonOf(error)}`)
    }
    // An error of the updater while the preparation ran stays.
    const now = this.current
    if (now.kind !== 'preparing' || now.version !== version) return
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
