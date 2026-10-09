// The two breakpoints the shell branches on. The phone has four tabs,
// pushed screens and sheets. Below the compact one the inspector is a column that the
// person opens and closes, so the Desk Panel does not open by itself.
// The layout below a breakpoint differs in structure, not only in
// style, so the shell reads the query rather than the stylesheet.

import { useCallback, useSyncExternalStore } from 'react'

export const MOBILE_QUERY = '(max-width: 760px)'
export const COMPACT_QUERY = '(max-width: 1100px)'

function query(media: string): MediaQueryList | null {
  return typeof window.matchMedia === 'function' ? window.matchMedia(media) : null
}

/** Whether the media query matches now, kept live across changes. */
export function useMediaQuery(media: string): boolean {
  const subscribe = useCallback(
    (onChange: () => void) => {
      const list = query(media)
      if (list === null) return () => undefined
      list.addEventListener('change', onChange)
      return () => list.removeEventListener('change', onChange)
    },
    [media],
  )
  return useSyncExternalStore(
    subscribe,
    () => query(media)?.matches ?? false,
    () => false,
  )
}

export function useIsMobile(): boolean {
  return useMediaQuery(MOBILE_QUERY)
}

/** True at 1100px and below, which holds the phone. */
export function useIsCompact(): boolean {
  return useMediaQuery(COMPACT_QUERY)
}
