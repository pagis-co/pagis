// The activity of the Person in this client (ADR-0030). After input in
// a visible document, the client sends one `activity` frame, at most
// once every 30 s. A new Needs-You item waits while the Person is
// active, so the phone does not ring while the Person reads Pagis here.
//
// Only input counts. The heartbeat `ping` and the query refetches also
// come from a hidden tab, so they do not tell that the Person is here.

/** The shortest time between two `activity` frames. */
export const ACTIVITY_INTERVAL_MS = 30_000

/**
 * Call `send` after a `pointerdown`, a `keydown`, a `focus` or a return
 * to the visible document, while the document is visible, at most once
 * every {@link ACTIVITY_INTERVAL_MS}. `send` answers whether the frame
 * went; a frame that did not go, as on a socket that is not online,
 * starts no interval, so the next input tries again. Answers the
 * function that stops it.
 */
export function watchActivity(send: () => boolean): () => void {
  let lastSentAt: number | null = null
  const onInput = () => {
    if (document.visibilityState !== 'visible') return
    const now = Date.now()
    if (lastSentAt !== null && now - lastSentAt < ACTIVITY_INTERVAL_MS) return
    if (send()) lastSentAt = now
  }
  // The capture phase sees an event that a component stops.
  window.addEventListener('pointerdown', onInput, { capture: true, passive: true })
  window.addEventListener('keydown', onInput, { capture: true })
  window.addEventListener('focus', onInput)
  document.addEventListener('visibilitychange', onInput)
  return () => {
    window.removeEventListener('pointerdown', onInput, { capture: true })
    window.removeEventListener('keydown', onInput, { capture: true })
    window.removeEventListener('focus', onInput)
    document.removeEventListener('visibilitychange', onInput)
  }
}
