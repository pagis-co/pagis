// An error that no code of the main process catches. Electron shows such
// an error in a native dialog with the raw JavaScript stack, unless the
// process has an `uncaughtException` listener. The client listens, and
// gives the failure to the setup page.

import { EventEmitter } from 'node:events'

import { describe, expect, it, vi } from 'vitest'

import { reportUncaughtExceptions } from './uncaughtFailure'

describe('an uncaught exception of the main process', () => {
  it('goes to the setup page as a failure, without the credential', () => {
    const process = new EventEmitter()
    const show = vi.fn()
    vi.spyOn(console, 'error').mockImplementation(() => {})

    reportUncaughtExceptions(process, show)
    process.emit('uncaughtException', new Error(`the download stopped; credential=${'a'.repeat(64)}`))

    expect(process.listenerCount('uncaughtException')).toBe(1)
    expect(show).toHaveBeenCalledWith(expect.any(Error))
    const shown = show.mock.calls[0]?.[0] as Error
    expect(shown.message).toContain('the download stopped')
    expect(shown.message).not.toContain('a'.repeat(64))
  })
})
