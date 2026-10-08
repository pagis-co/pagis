// The native shell of the Mobile App (`mobile/`), which loads the Product
// App of its server in a web view (ADR-0032). Its plugin `PagisShell`
// answers the main frame of the server alone.

import { Capacitor, registerPlugin, type PluginListenerHandle } from '@capacitor/core'
import { useNavigate } from '@tanstack/react-router'
import { useEffect } from 'react'
import { App } from '@capacitor/app'
import { useLocation, useSearch } from '@tanstack/react-router'
import { useIsMobile } from './state/useIsMobile'

/** The event of a tap on a Notification while the page is open: the
 *  path, the query and the fragment of the place of its item. */
interface ShellNavigateEvent {
  path?: unknown
}

interface PagisShellPlugin {
  changeServer(): Promise<void>
  setLockScreenAnswers(options: { on: boolean }): Promise<void>
  getLockScreenAnswers(): Promise<{ on: boolean }>
  /** The Session ended. The shell deletes its copy of the Session and
   *  opens its Connect screen. */
  sessionEnded(): Promise<void>
  addListener(
    eventName: 'navigate',
    listener: (event: ShellNavigateEvent) => void,
  ): Promise<PluginListenerHandle>
}

export const PagisShell = registerPlugin<PagisShellPlugin>('PagisShell')

export function phoneParent(path: string, from?: string): { label: string; path: string } {
  if (from && (path.endsWith('/desk') || path === '/memory')) return { label: from.startsWith('/runs/') ? 'Run' : from.startsWith('/c/') ? 'Conversation' : 'Sprite', path: from }
  const sprite = path.match(/^\/sprites\/([^/]+)(.*)$/)
  if (sprite) return sprite[2] === '' ? { label: 'Sprites', path: '/sprites' } : { label: sprite[2].startsWith('/access/') ? 'Access' : 'Sprite', path: `/sprites/${sprite[1]}${sprite[2].startsWith('/access/') ? '/access' : ''}` }
  const thread = path.match(/^(\/c\/[^/]+)\/t\//)
  if (thread) return { label: 'Conversation', path: thread[1] }
  if (path.startsWith('/c/')) return { label: 'Conversations', path: '/conversations' }
  if (path.startsWith('/runs/')) return { label: 'Home', path: from ?? '/' }
  if (path.startsWith('/calls/')) return { label: 'Home', path: '/' }
  if (path.startsWith('/settings/connections/')) return { label: 'Connections', path: '/settings/connections' }
  if (path === '/settings/trusted-contacts/keypad') return { label: 'Trusted contacts', path: '/settings/trusted-contacts' }
  if (path.startsWith('/settings/')) return { label: 'Settings', path: '/settings' }
  if (['/settings', '/memory', '/automations', '/software'].includes(path)) return { label: 'You', path: '/you' }
  return { label: 'Home', path: '/' }
}

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
  const location = useLocation()
  const phone = useIsMobile()
  const search = useSearch({ strict: false }) as { request?: string; new?: string; from?: string; rule?: unknown; package?: string; path?: string }

  useEffect(() => {
    if (!phone || Capacitor.getPlatform() !== 'android') return
    const listening = App.addListener('backButton', () => {
      if (document.querySelector('[role="dialog"], [role="alertdialog"]')) {
        document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
        return
      }
      if (search.request || search.new) { void navigate({ to: '.', search: (previous) => ({ ...previous, request: undefined, new: undefined }) }); return }
      if (search.rule || search.package || search.path) { void navigate({ to: '.', search: (previous) => ({ ...previous, rule: undefined, package: undefined, path: undefined }) }); return }
      if (location.pathname !== '/') void navigate({ href: phoneParent(location.pathname, search.from).path })
    })
    return () => { void listening.then((handle) => handle.remove()) }
  }, [phone, location.pathname, search.request, search.new, search.from, search.rule, search.package, search.path, navigate])

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
