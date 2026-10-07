// The activity of the Person: input in a visible document sends one
// `activity` frame, at most once every 30 s, and a hidden document
// sends none.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ACTIVITY_INTERVAL_MS, watchActivity } from './activity'

let visibility: DocumentVisibilityState = 'visible'

function setVisibility(state: DocumentVisibilityState) {
  visibility = state
  document.dispatchEvent(new Event('visibilitychange'))
}

function pointerdown() {
  window.dispatchEvent(new Event('pointerdown'))
}

describe('watchActivity', () => {
  let sent = 0
  let online = true
  let stop: () => void = () => {}

  beforeEach(() => {
    vi.useFakeTimers()
    visibility = 'visible'
    vi.spyOn(document, 'visibilityState', 'get').mockImplementation(() => visibility)
    sent = 0
    online = true
    stop = watchActivity(() => {
      if (!online) return false
      sent += 1
      return true
    })
  })

  afterEach(() => {
    stop()
    vi.restoreAllMocks()
    vi.useRealTimers()
  })

  it('sends at most one frame every 30 s', () => {
    pointerdown()
    expect(sent).toBe(1)

    vi.advanceTimersByTime(ACTIVITY_INTERVAL_MS - 1)
    pointerdown()
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'a' }))
    expect(sent).toBe(1)

    vi.advanceTimersByTime(1)
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'a' }))
    expect(sent).toBe(2)
  })

  it('counts a keydown, a focus and a return to the visible document', () => {
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'a' }))
    expect(sent).toBe(1)

    vi.advanceTimersByTime(ACTIVITY_INTERVAL_MS)
    window.dispatchEvent(new FocusEvent('focus'))
    expect(sent).toBe(2)

    vi.advanceTimersByTime(ACTIVITY_INTERVAL_MS)
    setVisibility('hidden')
    expect(sent).toBe(2)
    setVisibility('visible')
    expect(sent).toBe(3)
  })

  it('sends no frame from a hidden document', () => {
    setVisibility('hidden')

    pointerdown()
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'a' }))
    window.dispatchEvent(new FocusEvent('focus'))

    expect(sent).toBe(0)
  })

  it('tries again at the next input when a frame did not go', () => {
    online = false
    pointerdown()
    expect(sent).toBe(0)

    online = true
    pointerdown()
    expect(sent).toBe(1)
  })

  it('sends nothing after it stops', () => {
    stop()

    pointerdown()

    expect(sent).toBe(0)
  })
})
