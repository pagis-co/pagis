/**
 * Where each window of the Client App can go.
 *
 * Each window stays where the Client App loaded it: the product window on
 * its Product App origin, the administration window on the origin of the
 * Administration Port, and the setup and status windows on their packaged
 * page. This is the Electron security checklist item "Disable or limit
 * navigation".
 *
 * A page starts a navigation, and Electron emits `will-navigate` for it.
 * A server can answer a navigation with a redirect, and Electron emits
 * `will-redirect` for it. `will-navigate` sees only the first URL of a
 * navigation. Without `will-redirect`, a same-origin link to a route that
 * answers `302 Location: https://attacker.example/` moves the window to
 * that page, and the window has no address bar that shows the change.
 * `will-frame-navigate` does not replace the two events, because Electron
 * does not emit it for a redirect. One rule for each window decides both
 * events, because a redirect is a navigation that the server chose.
 *
 * The rule refuses each other address. The product and administration
 * windows send a refused address that `opensInSystemBrowser` allows to the
 * system browser, as they do with a new window that a page asks for. The
 * Person then sees the real address in a browser with an address bar. The
 * setup and status pages hold no link, so their windows send nothing there.
 */

import { opensInSystemBrowser } from './origin'
import { sameProductOrigin } from './recoveryView'

/**
 * Where one window of the Client App stays. The product window stays on
 * its Product App origin, and the administration window on the origin of
 * the Administration Port. Each origin is null while its window is not
 * open. The setup and status windows stay on their packaged page.
 */
export type WindowRule =
  | { window: 'product'; origin: string | null }
  | { window: 'administration'; origin: string | null }
  | { window: 'setup'; page: string }
  | { window: 'status'; page: string }

/** What the Client App does with a navigation or a redirect. */
export type NavigationAction = 'proceed' | 'open-in-browser' | 'refuse'

/** What Electron gives with `will-navigate` and `will-redirect`. */
export interface NavigationDetails {
  url: string
  isMainFrame: boolean
  preventDefault(): void
}

/** The part of a WebContents that the navigation rule listens on. */
export interface NavigatingContents {
  on(event: 'will-navigate' | 'will-redirect', listener: (details: NavigationDetails) => void): unknown
}

/**
 * What the Client App does when the main frame of a window goes to an
 * address, by a navigation that the page starts or by a server redirect.
 * A sub-frame does not move the window, so it proceeds. The Product App
 * decides which frames it shows, and the permission rule denies each
 * sub-frame.
 */
export function navigationAction(
  rule: WindowRule,
  navigation: { url: string; isMainFrame: boolean },
): NavigationAction {
  if (!navigation.isMainFrame) return 'proceed'
  const target = navigation.url
  if (rule.window === 'setup' || rule.window === 'status') {
    return target === rule.page ? 'proceed' : 'refuse'
  }
  if (rule.origin === null) return 'refuse'
  if (sameProductOrigin(rule.origin, target)) return 'proceed'
  // Only the product window shows a Product App, so only it opens the
  // loopback `http:` address of its own server in the system browser.
  const productOrigin = rule.window === 'product' ? rule.origin : null
  return opensInSystemBrowser(target, productOrigin) ? 'open-in-browser' : 'refuse'
}

/**
 * Put the navigation rule on the WebContents of one window. `rule` gives
 * the rule at each event, because a Local Installation that recovers can
 * move the product window to another port. `openInBrowser` opens an
 * address in the system browser.
 */
export function installNavigationRule(
  contents: NavigatingContents,
  rule: () => WindowRule,
  openInBrowser: (url: string) => void,
): void {
  const decide = (details: NavigationDetails): void => {
    const action = navigationAction(rule(), details)
    if (action === 'proceed') return
    details.preventDefault()
    if (action === 'open-in-browser') openInBrowser(details.url)
  }
  contents.on('will-navigate', decide)
  contents.on('will-redirect', decide)
}
