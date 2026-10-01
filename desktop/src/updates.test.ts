// The Update of the Client App on macOS (ADR-0027): electron-updater
// checks and downloads, Squirrel.Mac checks the new bundle, and the menus
// read the state.

import { EventEmitter } from 'node:events'

import type { UpdateCheckResult } from 'electron-updater'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { CHECK_EVERY_MS, Updates, type UpdateState } from './updates'

/** electron-updater, as the Client App uses it. Each check answers with
 *  the next result in `answers`, and emits the events of electron-updater
 *  for that result. */
class FakeUpdater extends EventEmitter {
  autoDownload = false
  autoInstallOnAppQuit = false
  allowPrerelease = true
  checks = 0
  installs = 0
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

  progress(percent: number): void {
    this.emit('download-progress', { percent, total: 100, transferred: percent, delta: 1, bytesPerSecond: 1 })
  }

  downloaded(version: string): void {
    this.emit('update-downloaded', { version, downloadedFile: '/tmp/update.zip' })
  }
}

function newUpdates() {
  const updater = new FakeUpdater()
  const squirrel = new EventEmitter()
  const states: UpdateState[] = []
  const notify = vi.fn()
  const updates = new Updates({
    updater,
    installer: squirrel,
    onState: (state) => states.push(state),
    notify,
  })
  return { updater, squirrel, states, notify, updates }
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
})
