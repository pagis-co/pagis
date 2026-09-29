// The navigation rule of the Client App windows. Each window stays on the
// origin or the page that the Client App loaded into it. A server redirect
// follows the same rule as a navigation that the page starts, because a
// redirect is a navigation that the server chose. An `https:` address that
// the rule refuses opens in the system browser, which has an address bar.

import { describe, expect, it } from 'vitest'

import {
  installNavigationRule,
  navigationAction,
  type NavigatingContents,
  type NavigationDetails,
  type WindowRule,
} from './windowNavigation'

/** The Product App origin of a Local Installation, as the Client App keeps it. */
const LOCAL = 'http://127.0.0.1:4400'
/** The Product App origin of a connected Server. */
const SERVER = 'https://pagis.example.com'
/** The Administration Port of the Local Installation. */
const ADMINISTRATION = 'http://127.0.0.1:4401'
/** The packaged pages of the setup and status windows. */
const SETUP_PAGE = 'file:///Applications/Pagis.app/Contents/Resources/app.asar/static/setup.html'
const STATUS_PAGE = 'file:///Applications/Pagis.app/Contents/Resources/app.asar/static/status.html'

const product: WindowRule = { window: 'product', origin: LOCAL }
const server: WindowRule = { window: 'product', origin: SERVER }
const administration: WindowRule = { window: 'administration', origin: ADMINISTRATION }
const setup: WindowRule = { window: 'setup', page: SETUP_PAGE }
const status: WindowRule = { window: 'status', page: STATUS_PAGE }

/** A navigation or a redirect of the main frame to `url`. */
function mainFrame(url: string) {
  return { url, isMainFrame: true }
}

describe('a navigation or a redirect in the product window', () => {
  it('refuses a redirect from the Product App origin to another origin, and allows a same-origin redirect', () => {
    // The sign-in link route answers `303 Location: /`.
    expect(navigationAction(product, mainFrame(`${LOCAL}/`))).toBe('proceed')
    expect(navigationAction(product, mainFrame(`${LOCAL}/channels/01ABC`))).toBe('proceed')
    expect(navigationAction(server, mainFrame(`${SERVER}/channels/01ABC`))).toBe('proceed')

    expect(navigationAction(product, mainFrame('https://example.com/'))).not.toBe('proceed')
    expect(navigationAction(server, mainFrame('https://attacker.example/sign-in'))).not.toBe('proceed')
  })

  /** The window has no address bar. The system browser shows the Person
   *  the real address, as for a new window that a page opens. */
  it('opens a refused https: or mailto: address in the system browser', () => {
    expect(navigationAction(product, mainFrame('https://example.com/'))).toBe('open-in-browser')
    expect(navigationAction(server, mainFrame('https://attacker.example/sign-in'))).toBe('open-in-browser')
    expect(navigationAction(server, mainFrame('https://pagis.example.com.attacker.example/'))).toBe('open-in-browser')
    expect(navigationAction(server, mainFrame('https://pagis.example.com:8443/'))).toBe('open-in-browser')
    expect(navigationAction(product, mainFrame('mailto:ada@example.com'))).toBe('open-in-browser')
  })

  /** The start route of a Google authorization answers with a redirect to
   *  Google. The Person consents in the system browser. */
  it('sends the Google consent page to the system browser', () => {
    const google = 'https://accounts.google.com/o/oauth2/v2/auth?client_id=pagis&state=Zx9-abc_123'
    expect(navigationAction(product, mainFrame(google))).toBe('open-in-browser')
  })

  /** Nothing authenticates the server of another `http:` address, so the
   *  system browser does not open it either. */
  it('refuses an address that the system browser does not open', () => {
    // Another computer, and another program on this machine.
    expect(navigationAction(product, mainFrame('http://192.168.1.10:4400/'))).toBe('refuse')
    expect(navigationAction(product, mainFrame('http://127.0.0.1:8080/'))).toBe('refuse')
    // A downgrade from TLS on the same host.
    expect(navigationAction(server, mainFrame('http://pagis.example.com/'))).toBe('refuse')
    // Another scheme, and text that is not a URL.
    expect(navigationAction(product, mainFrame('file:///etc/passwd'))).toBe('refuse')
    expect(navigationAction(product, mainFrame('javascript:alert(1)'))).toBe('refuse')
    expect(navigationAction(product, mainFrame('not a url'))).toBe('refuse')
  })

  it('refuses each address while the product window has no Product App origin', () => {
    const closed: WindowRule = { window: 'product', origin: null }
    expect(navigationAction(closed, mainFrame(`${LOCAL}/`))).toBe('refuse')
    expect(navigationAction(closed, mainFrame('https://example.com/'))).toBe('refuse')
  })

  /** A sub-frame does not move the window. The Product App decides which
   *  frames it shows, and the permission rule denies each sub-frame. */
  it('lets a sub-frame go to another origin', () => {
    expect(navigationAction(product, { url: 'https://example.com/', isMainFrame: false })).toBe('proceed')
  })
})

describe('a navigation or a redirect in the administration window', () => {
  it('stays on the origin of the Administration Port', () => {
    expect(navigationAction(administration, mainFrame(`${ADMINISTRATION}/settings`))).toBe('proceed')
  })

  /** The administration window shows no Product App, so it opens no
   *  loopback `http:` address in the system browser. */
  it('refuses another origin, and opens an https: address in the system browser', () => {
    expect(navigationAction(administration, mainFrame(`${LOCAL}/`))).toBe('refuse')
    expect(navigationAction(administration, mainFrame('http://192.168.1.10:4401/'))).toBe('refuse')
    expect(navigationAction(administration, mainFrame('https://example.com/'))).toBe('open-in-browser')
  })

  it('refuses each address while the window has no origin', () => {
    const closed: WindowRule = { window: 'administration', origin: null }
    expect(navigationAction(closed, mainFrame(`${ADMINISTRATION}/settings`))).toBe('refuse')
  })
})

describe('a navigation or a redirect in the setup and status windows', () => {
  it('stays on the packaged page of the window', () => {
    expect(navigationAction(setup, mainFrame(SETUP_PAGE))).toBe('proceed')
    expect(navigationAction(status, mainFrame(STATUS_PAGE))).toBe('proceed')
  })

  /** The setup and status pages hold no link to another address. */
  it('refuses each other address, and opens none in the system browser', () => {
    for (const rule of [setup, status]) {
      expect(navigationAction(rule, mainFrame('file:///etc/passwd'))).toBe('refuse')
      expect(navigationAction(rule, mainFrame(`${LOCAL}/`))).toBe('refuse')
      expect(navigationAction(rule, mainFrame('https://example.com/'))).toBe('refuse')
    }
    expect(navigationAction(setup, mainFrame(STATUS_PAGE))).toBe('refuse')
  })
})

type Listener = (details: NavigationDetails) => void

/** A WebContents that keeps the listeners that the Client App adds to it. */
function fakeContents() {
  const listeners = new Map<string, Listener[]>()
  const contents: NavigatingContents = {
    on: (event, listener) => {
      listeners.set(event, [...(listeners.get(event) ?? []), listener])
    },
  }
  return { contents, listeners }
}

/** Emit an event as Electron does. The result tells whether a listener
 *  prevented the navigation. */
function emit(listeners: Map<string, Listener[]>, event: string, url: string, isMainFrame = true): boolean {
  let prevented = false
  for (const listener of listeners.get(event) ?? []) {
    listener({ url, isMainFrame, preventDefault: () => { prevented = true } })
  }
  return prevented
}

describe('the navigation handlers of the Client App windows', () => {
  /** The address in each window that the rule keeps. */
  const windows: [WindowRule, string][] = [
    [product, `${LOCAL}/threads`],
    [administration, `${ADMINISTRATION}/settings`],
    [setup, SETUP_PAGE],
    [status, STATUS_PAGE],
  ]

  it('registers the redirect handler on the product, administration, setup and status windows', () => {
    for (const [rule, own] of windows) {
      const { contents, listeners } = fakeContents()

      installNavigationRule(contents, () => rule, () => {})

      expect(listeners.get('will-redirect'), rule.window).toHaveLength(1)
      expect(emit(listeners, 'will-redirect', 'https://attacker.example/'), rule.window).toBe(true)
      expect(emit(listeners, 'will-redirect', own), rule.window).toBe(false)
    }
  })

  it('registers the navigation handler with the same rule on each window', () => {
    for (const [rule, own] of windows) {
      const { contents, listeners } = fakeContents()

      installNavigationRule(contents, () => rule, () => {})

      expect(listeners.get('will-navigate'), rule.window).toHaveLength(1)
      expect(emit(listeners, 'will-navigate', 'https://attacker.example/'), rule.window).toBe(true)
      expect(emit(listeners, 'will-navigate', own), rule.window).toBe(false)
    }
  })

  /** The attack: a same-origin link to a route that answers
   *  `302 Location: https://example.com/`. */
  it('keeps the product window on its origin, and opens the redirect target in the system browser', () => {
    const { contents, listeners } = fakeContents()
    const opened: string[] = []

    installNavigationRule(contents, () => product, (url) => opened.push(url))

    expect(emit(listeners, 'will-navigate', `${LOCAL}/api/v1/redirect`)).toBe(false)
    expect(emit(listeners, 'will-redirect', 'https://example.com/')).toBe(true)
    expect(opened).toEqual(['https://example.com/'])
  })

  it('opens no refused address that the system browser does not open', () => {
    const { contents, listeners } = fakeContents()
    const opened: string[] = []

    installNavigationRule(contents, () => product, (url) => opened.push(url))

    expect(emit(listeners, 'will-redirect', 'http://192.168.1.10:4400/')).toBe(true)
    expect(emit(listeners, 'will-navigate', 'file:///etc/passwd')).toBe(true)
    expect(emit(listeners, 'will-redirect', 'https://example.com/', false)).toBe(false)
    expect(opened).toEqual([])
  })

  /** A Local Installation that recovers on another port moves the product
   *  window to that port. */
  it('reads the rule of the window again at each event', () => {
    const { contents, listeners } = fakeContents()
    let rule: WindowRule = product

    installNavigationRule(contents, () => rule, () => {})
    expect(emit(listeners, 'will-redirect', 'http://127.0.0.1:4402/')).toBe(true)

    rule = { window: 'product', origin: 'http://127.0.0.1:4402' }
    expect(emit(listeners, 'will-redirect', 'http://127.0.0.1:4402/')).toBe(false)
    expect(emit(listeners, 'will-redirect', `${LOCAL}/`)).toBe(true)
  })
})
