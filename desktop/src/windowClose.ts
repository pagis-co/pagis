export interface Hideable {
  hide(): void
}

export interface Cancellable {
  preventDefault(): void
}

/**
 * What a window close does (ADR-0025). The app stays in the tray
 * with the daemon running, because Schedules and mail wake-ups need
 * it. Only Quit closes the window for good.
 */
export function handleWindowClose(
  event: Cancellable,
  quitting: boolean,
  window: Hideable,
): 'hidden' | 'closed' {
  if (quitting) {
    return 'closed'
  }
  event.preventDefault()
  window.hide()
  return 'hidden'
}
