// The Update of the Client App (ADR-0027): electron-updater checks and
// downloads, Squirrel.Mac on macOS or the signed checksum list on Linux
// checks the download, and the menus read the state.

import { EventEmitter } from 'node:events'

import type { UpdateCheckResult, UpdateDownloadedEvent } from 'electron-updater'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { CHECK_EVERY_MS, type Installer, Updates, type UpdateState } from './updates'

/** electron-updater, as the Client App uses it. Each check answers with
 *  the next result in `answers`, and emits the events of electron-updater
 *  for that result. */
class FakeUpdater extends EventEmitter {
  autoDownload = false
  autoInstallOnAppQuit = false
  allowPrerelease = true
  checks = 0
  installs = 0
  /** The arguments of each `install`, which installs and does not quit. */
  installCalls: [boolean, boolean][] = []
  /** Where `install` moves the AppImage file, or null when the file keeps
   *  its name. */
  movesTo: string | null = null
  /** Whether `install` fails. electron-updater then emits `error`. */
  installFails = false
  answers: (string | null | Error)[] = []

  async checkForUpdates(): Promise<UpdateCheckResult | null> {
    this.checks += 1
    const answer = this.answers.shift() ?? null
    this.emit('checking-for-update')
    if (answer instanceof Error) {
      this.emit('error', answer)
      throw answer
    }
    if (answer === null) {
      this.emit('update-not-available', { version: '1.0.0' })
      return { isUpdateAvailable: false, updateInfo: { version: '1.0.0' } } as UpdateCheckResult
    }
    this.emit('update-available', { version: answer })
    return {
      isUpdateAvailable: true,
      updateInfo: { version: answer },
      downloadPromise: new Promise(() => undefined),
    } as unknown as UpdateCheckResult
  }

  quitAndInstall(): void {
    this.installs += 1
  }

  install(isSilent: boolean, isForceRunAfter: boolean): boolean {
    this.installCalls.push([isSilent, isForceRunAfter])
    if (this.installFails) {
      this.emit('error', new Error('EACCES: permission denied, rename'))
      return false
    }
    if (this.movesTo !== null) this.emit('appimage-filename-updated', this.movesTo)
    return true
  }

  progress(percent: number): void {
    this.emit('download-progress', { percent, total: 100, transferred: percent, delta: 1, bytesPerSecond: 1 })
  }

  downloaded(version: string, downloadedFile = '/tmp/update.zip'): void {
    this.emit('update-downloaded', { version, downloadedFile })
  }
}

function newUpdates(installer: (squirrel: EventEmitter) => Installer = (squirrel) => ({ kind: 'squirrel', squirrel })) {
  const updater = new FakeUpdater()
  const squirrel = new EventEmitter()
  const states: UpdateState[] = []
  const notify = vi.fn()
  const updates = new Updates({
    updater,
    installer: installer(squirrel),
    onState: (state) => states.push(state),
    notify,
  })
  return { updater, squirrel, states, notify, updates }
}

const APPIMAGE = '/home/me/Apps/Pagis-1.0.0-x86_64.AppImage'

/** The AppImage installer. `restarts` holds the file of each restart. */
function appImage(verify: (update: UpdateDownloadedEvent) => Promise<void>) {
  const restarts: string[] = []
  const installer: Installer = { kind: 'appimage', verify, file: APPIMAGE, restart: (file) => { restarts.push(file) } }
  return { installer, restarts }
}

/** An AppImage Update of 1.1.0 that passed its check. */
async function readyAppImage() {
  const check = verification()
  const { installer, restarts } = appImage(check.verify)
  const { updates, updater } = newUpdates(() => installer)
  updater.answers = ['1.1.0']
  await updates.check()
  updater.downloaded('1.1.0', '/cache/pending/Pagis-1.1.0-x86_64.AppImage')
  await check.pass()
  expect(updates.state).toEqual({ kind: 'ready', version: '1.1.0' })
  return { updates, updater, restarts }
}

/** The check of a Linux download, which the test settles. */
function verification() {
  const checked: UpdateDownloadedEvent[] = []
  let settle: { resolve(): void; reject(error: Error): void } | null = null
  const verify = (update: UpdateDownloadedEvent) => {
    checked.push(update)
    return new Promise<void>((resolve, reject) => { settle = { resolve, reject } })
  }
  return {
    checked,
    verify,
    pass: async () => { settle?.resolve(); await vi.advanceTimersByTimeAsync(0) },
    fail: async (error: Error) => { settle?.reject(error); await vi.advanceTimersByTimeAsync(0) },
  }
}

describe('the Update of the Client App', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('downloads an Update with no question, installs it at quit, and takes no prerelease', () => {
    const { updater } = newUpdates()

    expect(updater.autoDownload).toBe(true)
    expect(updater.autoInstallOnAppQuit).toBe(true)
    expect(updater.allowPrerelease).toBe(false)
  })

  /** electron-updater would install each Linux download at quit, before
   *  the check of the signed checksum list. */
  it('does not let electron-updater install a download at quit on Linux', () => {
    const verify = async () => undefined
    for (const installer of [appImage(verify).installer, { kind: 'deb', verify } as const]) {
      const { updater } = newUpdates(() => installer)

      expect(updater.autoDownload).toBe(true)
      expect(updater.autoInstallOnAppQuit).toBe(false)
      expect(updater.allowPrerelease).toBe(false)
    }
  })

  it('says that the client is up to date when no Update is out', async () => {
    const { updates, states } = newUpdates()

    const result = await updates.check()

    expect(result).toEqual({ kind: 'up-to-date' })
    expect(states).toEqual([{ kind: 'checking' }, { kind: 'idle' }])
  })

  it('downloads an Update that it finds and reports the progress in whole percent', async () => {
    const { updates, updater, states } = newUpdates()
    updater.answers = ['1.1.0']

    const result = await updates.check()
    updater.progress(12.2)
    updater.progress(12.9)
    updater.progress(40.5)

    expect(result).toEqual({ kind: 'found', version: '1.1.0' })
    expect(states).toEqual([
      { kind: 'checking' },
      { kind: 'downloading', version: '1.1.0', percent: 0 },
      { kind: 'downloading', version: '1.1.0', percent: 12 },
      { kind: 'downloading', version: '1.1.0', percent: 40 },
    ])
  })

  it('is ready only when Squirrel.Mac accepted the downloaded bundle, and notifies one time', async () => {
    const { updates, updater, squirrel, notify } = newUpdates()
    updater.answers = ['1.1.0']
    await updates.check()

    updater.downloaded('1.1.0')
    expect(updates.state).toEqual({ kind: 'downloading', version: '1.1.0', percent: 100 })
    expect(notify).not.toHaveBeenCalled()

    squirrel.emit('update-downloaded')
    squirrel.emit('update-downloaded')

    expect(updates.state).toEqual({ kind: 'ready', version: '1.1.0' })
    expect(notify).toHaveBeenCalledTimes(1)
    expect(notify).toHaveBeenCalledWith('1.1.0')
  })

  it('fails when Squirrel.Mac refuses the bundle', async () => {
    const { updates, updater, notify } = newUpdates()
    updater.answers = ['1.1.0']
    await updates.check()
    updater.downloaded('1.1.0')

    updater.emit('error', new Error('Code signature at URL did not pass validation\nmore detail'))

    expect(updates.state).toEqual({ kind: 'failed', reason: 'Code signature at URL did not pass validation' })
    expect(notify).not.toHaveBeenCalled()
  })

  it('answers a failed check with the reason', async () => {
    const { updates, updater } = newUpdates()
    updater.answers = [new Error('net::ERR_INTERNET_DISCONNECTED')]

    const result = await updates.check()

    expect(result).toEqual({ kind: 'failed', reason: 'net::ERR_INTERNET_DISCONNECTED' })
    expect(updates.state).toEqual({ kind: 'failed', reason: 'net::ERR_INTERNET_DISCONNECTED' })
  })

  it('does not check again while an Update downloads or is ready', async () => {
    const { updates, updater, squirrel } = newUpdates()
    updater.answers = ['1.1.0']
    await updates.check()

    expect(await updates.check()).toEqual({ kind: 'found', version: '1.1.0' })
    updater.downloaded('1.1.0')
    squirrel.emit('update-downloaded')
    expect(await updates.check()).toEqual({ kind: 'ready', version: '1.1.0' })
    expect(updater.checks).toBe(1)
  })

  it('checks at start and then every 24 hours on the clock', async () => {
    const { updates, updater } = newUpdates()

    updates.start()
    await vi.advanceTimersByTimeAsync(0)
    expect(updater.checks).toBe(1)

    await vi.advanceTimersByTimeAsync(CHECK_EVERY_MS - 1)
    expect(updater.checks).toBe(1)
    await vi.advanceTimersByTimeAsync(60 * 60 * 1000)
    expect(updater.checks).toBe(2)
  })

  /** Node stops its timers while a Mac sleeps, so the client compares the
   *  time of the last check with the clock and does not count 24 hours of
   *  timer time. */
  it('checks after a sleep when 24 hours went by on the clock', async () => {
    const { updates, updater } = newUpdates()
    updates.start()
    await vi.advanceTimersByTimeAsync(0)

    vi.setSystemTime(Date.now() + CHECK_EVERY_MS)
    await vi.advanceTimersByTimeAsync(60 * 60 * 1000)

    expect(updater.checks).toBe(2)
  })

  it('starts one schedule, and stops it', async () => {
    const { updates, updater } = newUpdates()

    updates.start()
    updates.start()
    await vi.advanceTimersByTimeAsync(0)
    expect(updater.checks).toBe(1)

    updates.stop()
    await vi.advanceTimersByTimeAsync(2 * CHECK_EVERY_MS)
    expect(updater.checks).toBe(1)
  })

  it('installs only a ready Update', async () => {
    const { updates, updater, squirrel } = newUpdates()

    expect(() => updates.install()).toThrow(/no Update is ready/)

    updater.answers = ['1.1.0']
    await updates.check()
    updater.downloaded('1.1.0')
    squirrel.emit('update-downloaded')
    updates.install()

    expect(updater.installs).toBe(1)
  })

  it('is ready on Linux only when the download passed the signed checksum list, and notifies one time', async () => {
    const check = verification()
    const { updates, updater, notify } = newUpdates(() => appImage(check.verify).installer)
    updater.answers = ['1.1.0']
    await updates.check()

    updater.downloaded('1.1.0', '/cache/pending/Pagis-1.1.0-x86_64.AppImage')
    expect(check.checked).toEqual([{ version: '1.1.0', downloadedFile: '/cache/pending/Pagis-1.1.0-x86_64.AppImage' }])
    expect(updates.state).toEqual({ kind: 'downloading', version: '1.1.0', percent: 100 })
    expect(() => updates.install()).toThrow(/no Update is ready/)

    await check.pass()

    expect(updates.state).toEqual({ kind: 'ready', version: '1.1.0' })
    expect(notify).toHaveBeenCalledTimes(1)
    expect(notify).toHaveBeenCalledWith('1.1.0')
  })

  it('fails on Linux with the reason when the download does not pass, and installs nothing', async () => {
    const check = verification()
    const { updates, updater, notify } = newUpdates(() => appImage(check.verify).installer)
    updater.answers = ['1.1.0']
    await updates.check()
    updater.downloaded('1.1.0')

    await check.fail(new Error('Pagis-1.1.0-linux.SHA256SUMS does not verify with the Pagis release key'))

    expect(updates.state).toEqual({
      kind: 'failed',
      reason: 'Pagis-1.1.0-linux.SHA256SUMS does not verify with the Pagis release key',
    })
    expect(notify).not.toHaveBeenCalled()
    updates.installAtQuit()
    expect(updater.installCalls).toEqual([])
    expect(() => updates.install()).toThrow(/no Update is ready/)
  })

  it('keeps a failure that comes while the check runs', async () => {
    const check = verification()
    const { updates, updater, notify } = newUpdates(() => appImage(check.verify).installer)
    updater.answers = ['1.1.0']
    await updates.check()
    updater.downloaded('1.1.0')

    updater.emit('error', new Error('net::ERR_CONNECTION_RESET'))
    await check.pass()

    expect(updates.state).toEqual({ kind: 'failed', reason: 'net::ERR_CONNECTION_RESET' })
    expect(notify).not.toHaveBeenCalled()
  })

  /** At quit, the Client App makes the call that electron-updater makes
   *  when autoInstallOnAppQuit is on: a silent install, and no start. */
  it('installs a ready AppImage Update at quit, and only a ready one, and starts nothing', async () => {
    const check = verification()
    const { installer, restarts } = appImage(check.verify)
    const { updates, updater } = newUpdates(() => installer)

    updates.installAtQuit()
    updater.answers = ['1.1.0']
    await updates.check()
    updater.downloaded('1.1.0')
    updates.installAtQuit()
    expect(updater.installCalls).toEqual([])

    await check.pass()
    updater.movesTo = '/home/me/Apps/Pagis-1.1.0-x86_64.AppImage'
    updates.installAtQuit()

    expect(updater.installCalls).toEqual([[true, false]])
    expect(restarts).toEqual([])
  })

  /**
   * electron-updater would start the new AppImage before this process
   * ends. The new process then finds the single-instance lock taken and
   * exits, and Pagis does not come back. So the AppImage installs with no
   * start, and the new file starts after this process ended.
   */
  it('restarts into the new AppImage file after the install, and electron-updater starts nothing', async () => {
    const { updates, updater, restarts } = await readyAppImage()
    updater.movesTo = '/home/me/Apps/Pagis-1.1.0-x86_64.AppImage'

    updates.install()

    expect(updater.installCalls).toEqual([[true, false]])
    expect(updater.installs).toBe(0)
    expect(restarts).toEqual(['/home/me/Apps/Pagis-1.1.0-x86_64.AppImage'])
  })

  it('restarts the same AppImage file when the Update keeps its name', async () => {
    const { updates, restarts } = await readyAppImage()

    updates.install()

    expect(restarts).toEqual([APPIMAGE])
  })

  it('does not restart when the AppImage install fails', async () => {
    const { updates, updater, restarts } = await readyAppImage()
    updater.installFails = true

    updates.install()

    expect(restarts).toEqual([])
    expect(updates.state).toEqual({ kind: 'failed', reason: 'EACCES: permission denied, rename' })
  })

  it('does not install again at quit after Restart to Update', async () => {
    const { updates, updater, restarts } = await readyAppImage()

    updates.install()
    updates.installAtQuit()

    expect(updater.installCalls).toEqual([[true, false]])
    expect(restarts).toEqual([APPIMAGE])
  })

  /** A password prompt at quit or at logout stops the shutdown. */
  it('installs a deb only from Restart to Update, never at quit', async () => {
    const check = verification()
    const { updates, updater } = newUpdates(() => ({ kind: 'deb', verify: check.verify }))
    updater.answers = ['1.1.0']
    await updates.check()
    updater.downloaded('1.1.0', '/cache/pending/Pagis-1.1.0-amd64.deb')
    await check.pass()

    updates.installAtQuit()
    expect(updater.installCalls).toEqual([])

    updates.install()
    expect(updater.installs).toBe(1)
    expect(updater.installCalls).toEqual([])
  })

  /** Squirrel.Mac installs the Update that it accepted when the app quits. */
  it('leaves the install at quit to Squirrel.Mac on macOS', async () => {
    const { updates, updater, squirrel } = newUpdates()
    updater.answers = ['1.1.0']
    await updates.check()
    updater.downloaded('1.1.0')
    squirrel.emit('update-downloaded')

    updates.installAtQuit()

    expect(updater.installCalls).toEqual([])
  })
})
