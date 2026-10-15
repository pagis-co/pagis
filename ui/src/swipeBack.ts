// The edge swipe of iOS (ADR-0034). A drag from the left edge of a
// pushed screen goes back, as in the iOS navigation stack. The screen
// follows the finger. A release past a third of the width, or a fast
// flick, goes back; any other release springs the screen back. Android
// has its own back gesture, and the shell sends it as the back button.

import { useEffect, useRef, type RefObject } from 'react'

/** The band at the left edge, in CSS pixels, where the swipe starts. */
const EDGE = 24
/** The movement before the drag picks a direction. */
const SLOP = 10
/** The part of the width past which a release goes back. */
const COMMIT_PART = 1 / 3
/** The speed in pixels a millisecond past which a release goes back. */
const COMMIT_SPEED = 0.5

/** Whether a control between the target and the screen takes its own
 *  touches, as the live screen of a Desk and hold-to-talk do. */
function takesOwnTouches(target: EventTarget | null, screen: HTMLElement): boolean {
  for (let node = target instanceof Element ? target : null; node !== null && node !== screen; node = node.parentElement) {
    if (getComputedStyle(node).touchAction === 'none') return true
  }
  return false
}

function reducedMotion(): boolean {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false
}

/** Go back on an edge swipe over the element while `enabled` is true. */
export function useEdgeSwipeBack(ref: RefObject<HTMLElement | null>, onBack: () => void, enabled: boolean): void {
  const back = useRef(onBack)
  useEffect(() => {
    back.current = onBack
  })

  useEffect(() => {
    const screen = ref.current
    if (!enabled || screen === null) return
    // The last two places of the finger give the speed of the release.
    let drag: { x: number; y: number; dx: number; time: number; lastDx: number; lastTime: number; moving: boolean } | null = null

    const place = (dx: number, animate: boolean) => {
      screen.style.transition = animate && !reducedMotion() ? 'transform 200ms ease-out' : ''
      screen.style.transform = dx === 0 ? '' : `translateX(${dx}px)`
    }
    const onStart = (event: TouchEvent) => {
      const touch = event.touches[0]
      drag = null
      if (event.touches.length !== 1 || touch.clientX > EDGE || takesOwnTouches(event.target, screen)) return
      const now = performance.now()
      drag = { x: touch.clientX, y: touch.clientY, dx: 0, time: now, lastDx: 0, lastTime: now, moving: false }
    }
    const onMove = (event: TouchEvent) => {
      if (drag === null) return
      const touch = event.touches[0]
      const dx = touch.clientX - drag.x
      const dy = touch.clientY - drag.y
      if (!drag.moving) {
        if (Math.abs(dy) > SLOP && Math.abs(dy) > dx) {
          drag = null
          return
        }
        if (dx < SLOP) return
        drag.moving = true
      }
      // The screen moves, so the page does not scroll under the finger.
      event.preventDefault()
      drag.lastDx = drag.dx
      drag.lastTime = drag.time
      drag.dx = Math.max(0, dx)
      drag.time = performance.now()
      place(drag.dx, false)
    }
    const onEnd = () => {
      const done = drag
      drag = null
      if (done === null || !done.moving) return
      const speed = (done.dx - done.lastDx) / Math.max(1, done.time - done.lastTime)
      const commit = done.dx > screen.clientWidth * COMMIT_PART || speed > COMMIT_SPEED
      // The parent replaces the content, so the screen goes back to its
      // place at once; a cancelled swipe springs back.
      place(0, !commit)
      if (commit) back.current()
    }
    const onCancel = () => {
      if (drag?.moving) place(0, true)
      drag = null
    }

    screen.addEventListener('touchstart', onStart, { passive: true })
    screen.addEventListener('touchmove', onMove, { passive: false })
    screen.addEventListener('touchend', onEnd)
    screen.addEventListener('touchcancel', onCancel)
    return () => {
      screen.removeEventListener('touchstart', onStart)
      screen.removeEventListener('touchmove', onMove)
      screen.removeEventListener('touchend', onEnd)
      screen.removeEventListener('touchcancel', onCancel)
      place(0, false)
    }
  }, [ref, enabled])
}
