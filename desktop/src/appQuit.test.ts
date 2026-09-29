// How the Client App ends its process. A quit comes from the Quit button
// of a page, the Quit Pagis menu item, the Dock, a logout, or SIGTERM and
// SIGINT. Each one ends the process after the client stopped its work.

import { EventEmitter } from 'node:events'

import { describe, expect, it, vi } from 'vitest'

import { endOnQuit } from './appQuit'

/** An app that emits `before-quit` as Electron does, and records `exit`. */
class FakeApp extends EventEmitter {
  readonly exits: number[] = []

  exit(code = 0): void {
    this.exits.push(code)
  }

  /** A quit, as Electron starts it. Answers whether a listener held it. */
  quit(): boolean {
    let held = false
    this.emit('before-quit', { preventDefault: () => { held = true } })
    return held
  }
}

describe('a quit of the Client App', () => {
  it('holds the quit while the client stops, then ends the process', async () => {
    const app = new FakeApp()
    let finishStop!: () => void
    const stop = vi.fn(() => new Promise<void>((resolve) => { finishStop = resolve }))
    endOnQuit(app, stop)

    expect(app.quit()).toBe(true)
    await Promise.resolve()
    expect(stop).toHaveBeenCalledTimes(1)
    expect(app.exits).toEqual([])

    finishStop()
    await vi.waitFor(() => expect(app.exits).toEqual([0]))
  })

  /** Electron does not finish a second `app.quit()` after the app held a
   *  quit that a signal started: the windows close and the process stays.
   *  So the client ends the process itself, and a second quit, such as
   *  SIGTERM after Quit Pagis, joins the first one. */
  it('stops once and ends the process once for two quits', async () => {
    const app = new FakeApp()
    let finishStop!: () => void
    const stop = vi.fn(() => new Promise<void>((resolve) => { finishStop = resolve }))
    endOnQuit(app, stop)

    app.quit()
    expect(app.quit()).toBe(true)
    finishStop()

    await vi.waitFor(() => expect(app.exits).toEqual([0]))
    expect(stop).toHaveBeenCalledTimes(1)
  })

  it('ends the process with a failure code when the client does not stop cleanly', async () => {
    const app = new FakeApp()
    const error = vi.spyOn(console, 'error').mockImplementation(() => {})
    endOnQuit(app, async () => { throw new Error('the server did not stop') })

    app.quit()

    await vi.waitFor(() => expect(app.exits).toEqual([1]))
    expect(error).toHaveBeenCalledWith(expect.stringContaining('the server did not stop'))
    error.mockRestore()
  })
})
