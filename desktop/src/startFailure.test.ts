// A start of the client that failed. A Person reads the failure in a
// native message box. The smoke test has no Person to close the box, and
// the open box holds the process, so the smoke prints the failure and
// exits.

import { describe, expect, it, vi } from 'vitest'

import { endFailedStart } from './startFailure'

describe('a failed start', () => {
  it('shows the failure in a message box, then exits with 1', () => {
    const app = { exit: vi.fn() }
    const dialog = { showErrorBox: vi.fn() }

    endFailedStart(new Error('the lock is unreadable'), false, app, dialog)

    expect(dialog.showErrorBox).toHaveBeenCalledWith('Pagis could not start', 'Error: the lock is unreadable')
    expect(app.exit).toHaveBeenCalledWith(1)
  })

  it('in the smoke test prints the failure and exits with 1, with no message box', () => {
    const app = { exit: vi.fn() }
    const dialog = { showErrorBox: vi.fn() }
    const error = vi.spyOn(console, 'error').mockImplementation(() => {})

    endFailedStart(new Error('the lock is unreadable'), true, app, dialog)

    expect(dialog.showErrorBox).not.toHaveBeenCalled()
    expect(error).toHaveBeenCalledWith('pagis smoke: the client did not start: Error: the lock is unreadable')
    expect(app.exit).toHaveBeenCalledWith(1)
  })
})
