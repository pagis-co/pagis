// The native shell of the Mobile App (`mobile/`), which loads the Product
// App of its server in a web view (ADR-0032). Its plugin `PagisShell`
// answers the main frame of the server alone.

import { Capacitor, registerPlugin, type PluginListenerHandle } from '@capacitor/core'
import { useNavigate } from '@tanstack/react-router'
import { useEffect } from 'react'

/** The event of a tap on a Notification while the page is open: the
 *  path, the query and the fragment of the place of its item. */
interface ShellNavigateEvent {
  path?: unknown
}

interface PagisShellPlugin {
  /** The Session ended. The shell deletes its copy of the Session and
   *  opens its Connect screen. */
  sessionEnded(): Promise<void>
  addListener(
    eventName: 'navigate',
    listener: (event: ShellNavigateEvent) => void,
  ): Promise<PluginListenerHandle>
}

const PagisShell = registerPlugin<PagisShellPlugin>('PagisShell')

/** Tell the Mobile App that the Session of this page ended. A browser
 *  has no shell, and nothing happens there. */
export function reportSessionEnded(): void {
  if (!Capacitor.isNativePlatform()) return
  PagisShell.sessionEnded().catch((error: unknown) => {
    console.error('The Mobile App did not take the end of the Session.', error)
  })
}

/** The place of a `navigate` event as a path on the origin of the page,
 *  or `null` for a value that names no place there. */
function placeOf(event: ShellNavigateEvent): string | null {
  if (typeof event.path !== 'string') return null
  let url: URL
  try {
    url = new URL(event.path, window.location.origin)
  } catch {
    return null
  }
  if (url.origin !== window.location.origin) return null
  return `${url.pathname}${url.search}${url.hash}`
}

/** Move the router to the place that the Mobile App names after a tap on
 *  a Notification. The page does not load again, so it keeps its state.
 *  A browser has no shell, and the hook adds no listener there. */
export function useShellNavigation(): void {
  const navigate = useNavigate()

  useEffect(() => {
    if (!Capacitor.isNativePlatform()) return
    const listening = PagisShell.addListener('navigate', (event) => {
      const href = placeOf(event)
      if (href !== null) void navigate({ href })
    })
    return () => {
      listening.then(
        (handle) => handle.remove(),
        (error: unknown) => {
          console.error('The page did not listen for the taps on a Notification.', error)
        },
      )
    }
  }, [navigate])
}
