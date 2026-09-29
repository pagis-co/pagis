import { describe, expect, it, vi } from 'vitest'

import { handleWindowClose } from './windowClose'

describe('a window close', () => {
  it('keeps the app in the tray', () => {
    const event = { preventDefault: vi.fn() }
    const window = { hide: vi.fn() }

    expect(handleWindowClose(event, false, window)).toBe('hidden')
    expect(event.preventDefault).toHaveBeenCalled()
    expect(window.hide).toHaveBeenCalled()
  })

  it('closes the window when the user quits', () => {
    const event = { preventDefault: vi.fn() }
    const window = { hide: vi.fn() }

    expect(handleWindowClose(event, true, window)).toBe('closed')
    expect(event.preventDefault).not.toHaveBeenCalled()
    expect(window.hide).not.toHaveBeenCalled()
  })
})
