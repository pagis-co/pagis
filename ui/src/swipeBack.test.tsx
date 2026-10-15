// The edge swipe of iOS. A drag from the left edge of a pushed screen
// goes back, and the screen follows the finger while it moves.

import { fireEvent, render } from '@testing-library/react'
import { useRef, type ReactNode } from 'react'
import { describe, expect, it, vi } from 'vitest'

import { useEdgeSwipeBack } from './swipeBack'

function Screen({ onBack, enabled = true, children }: { onBack: () => void; enabled?: boolean; children?: ReactNode }) {
  const ref = useRef<HTMLElement>(null)
  useEdgeSwipeBack(ref, onBack, enabled)
  return <main ref={ref} data-testid="screen">{children}</main>
}

/** Mount a screen 390 px wide, as a phone draws it. */
function mount(onBack: () => void, options: { enabled?: boolean; children?: ReactNode } = {}): HTMLElement {
  const { getByTestId } = render(<Screen onBack={onBack} {...options} />)
  const screen = getByTestId('screen')
  Object.defineProperty(screen, 'clientWidth', { value: 390 })
  return screen
}

/** Drag one finger through the points, each `ms` after the one before. */
function drag(target: Element, points: [number, number][], ms = 50): void {
  const [first, ...rest] = points
  let time = 0
  const clock = vi.spyOn(performance, 'now').mockImplementation(() => time)
  const at = ([clientX, clientY]: [number, number]) => [{ clientX, clientY }]
  fireEvent.touchStart(target, { touches: at(first) })
  for (const point of rest) {
    time += ms
    fireEvent.touchMove(target, { touches: at(point) })
  }
  fireEvent.touchEnd(target, { touches: [] })
  clock.mockRestore()
}

describe('useEdgeSwipeBack', () => {
  it('goes back after a drag from the left edge past a third of the screen', () => {
    const onBack = vi.fn()
    const screen = mount(onBack)

    drag(screen, [[8, 300], [60, 302], [160, 305], [200, 306]])

    expect(onBack).toHaveBeenCalledTimes(1)
    expect(screen.style.transform).toBe('')
  })

  it('goes back after a fast flick from the left edge', () => {
    const onBack = vi.fn()
    const screen = mount(onBack)

    drag(screen, [[8, 300], [40, 300], [90, 300]], 10)

    expect(onBack).toHaveBeenCalledTimes(1)
  })

  it('moves the screen with the finger', () => {
    const screen = mount(vi.fn())

    fireEvent.touchStart(screen, { touches: [{ clientX: 8, clientY: 300 }] })
    fireEvent.touchMove(screen, { touches: [{ clientX: 120, clientY: 300 }] })

    expect(screen.style.transform).toBe('translateX(112px)')
  })

  it('springs back after a short slow drag', () => {
    const onBack = vi.fn()
    const screen = mount(onBack)

    drag(screen, [[8, 300], [30, 300], [60, 300]], 200)

    expect(onBack).not.toHaveBeenCalled()
    expect(screen.style.transform).toBe('')
  })

  it('does nothing for a drag that starts away from the edge', () => {
    const onBack = vi.fn()
    const screen = mount(onBack)

    drag(screen, [[100, 300], [200, 300], [300, 300]])

    expect(onBack).not.toHaveBeenCalled()
    expect(screen.style.transform).toBe('')
  })

  it('leaves a vertical scroll to the screen', () => {
    const onBack = vi.fn()
    const screen = mount(onBack)

    drag(screen, [[8, 300], [12, 360], [200, 420]])

    expect(onBack).not.toHaveBeenCalled()
  })

  // The live screen of a Desk and the hold-to-talk control take their
  // own touches with `touch-action: none`.
  it('leaves a drag to a control that takes its own touches', () => {
    const onBack = vi.fn()
    const screen = mount(onBack, { children: <div data-testid="canvas" style={{ touchAction: 'none' }} /> })

    drag(screen.querySelector('[data-testid="canvas"]')!, [[8, 300], [160, 300], [250, 300]])

    expect(onBack).not.toHaveBeenCalled()
  })

  it('does nothing when it is off', () => {
    const onBack = vi.fn()
    const screen = mount(onBack, { enabled: false })

    drag(screen, [[8, 300], [160, 300], [250, 300]])

    expect(onBack).not.toHaveBeenCalled()
  })
})
