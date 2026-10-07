// The state of this client for Notifications, from fake browser globals,
// the Push Subscription that the browser holds, and what the daemon says.

import { describe, expect, it } from 'vitest'

import { base64UrlBytes, pushState, type PushGlobals } from './support'

/** The daemon's VAPID public key: an uncompressed P-256 point. */
const KEY = Buffer.from([4, ...Array.from({ length: 64 }, (_, index) => index)]).toString(
  'base64url',
)
const OTHER_KEY = Buffer.from([4, ...Array.from({ length: 64 }, () => 9)]).toString('base64url')

const CHROME =
  'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36'
const CLIENT_APP =
  'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Pagis/0.1.1 Chrome/152.0.7977.65 Electron/44.1.1 Safari/537.36'
const IPHONE_SAFARI =
  'Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1'

/** A browser with a service worker, the Push API, and no answer yet to
 *  the notification permission. */
function browser(overrides: Partial<PushGlobals> = {}): PushGlobals {
  return {
    navigator: { userAgent: CHROME, serviceWorker: {} },
    PushManager: function PushManager() {},
    Notification: { permission: 'default' },
    ...overrides,
  }
}

/** A Push Subscription that the browser holds, made with `key`. */
function held(key: string) {
  return { options: { applicationServerKey: base64UrlBytes(key).buffer } }
}

const listed = { listed: true, key: KEY }
const notListed = { listed: false, key: KEY }

describe('pushState', () => {
  it('is Not available in a browser with no service worker', () => {
    const globals = browser({ navigator: { userAgent: CHROME } })
    expect(pushState(globals, null, notListed)).toBe('not-available')
  })

  it('is Not available in a browser with no Push API', () => {
    const globals = browser({ PushManager: undefined })
    expect(pushState(globals, null, notListed)).toBe('not-available')
  })

  // The Client App denies the permission, but it has no browser setting
  // to change, so it is not Blocked.
  it('is Not available in the Client App', () => {
    const globals = browser({
      navigator: { userAgent: CLIENT_APP, serviceWorker: {} },
      Notification: { permission: 'denied' },
    })
    expect(pushState(globals, null, notListed)).toBe('not-available')
  })

  // A Safari tab on iOS has a service worker but no Push API.
  it('is Add to Home Screen first in a Safari tab on iOS', () => {
    const globals = browser({
      navigator: { userAgent: IPHONE_SAFARI, serviceWorker: {}, standalone: false },
      PushManager: undefined,
    })
    expect(pushState(globals, null, notListed)).toBe('add-to-home-screen')
  })

  it('is Off in a Home Screen web app on iOS that holds no subscription', () => {
    const globals = browser({
      navigator: { userAgent: IPHONE_SAFARI, serviceWorker: {}, standalone: true },
    })
    expect(pushState(globals, null, notListed)).toBe('off')
  })

  it('is Blocked when the person denied the notification permission', () => {
    const globals = browser({ Notification: { permission: 'denied' } })
    expect(pushState(globals, null, notListed)).toBe('blocked')
  })

  it('is Off when the browser holds no subscription', () => {
    expect(pushState(browser(), null, listed)).toBe('off')
  })

  it('is On when the browser holds a subscription that the daemon lists, with its key', () => {
    expect(pushState(browser(), held(KEY), listed)).toBe('on')
  })

  it('is stale when the browser holds a subscription that the daemon does not list', () => {
    expect(pushState(browser(), held(KEY), notListed)).toBe('stale')
  })

  it('is stale when the browser holds a subscription with another key', () => {
    expect(pushState(browser(), held(OTHER_KEY), listed)).toBe('stale')
  })

  it('is the Mobile App where the native shell runs the page', () => {
    const globals = browser({
      PushManager: undefined,
      Capacitor: { isNativePlatform: () => true },
    })
    expect(pushState(globals, null, notListed)).toBe('mobile-app')
  })

  // `@capacitor/core` puts a `Capacitor` global on every page that loads
  // it, also in a browser.
  it('is not the Mobile App where Capacitor runs on the web', () => {
    const globals = browser({ Capacitor: { isNativePlatform: () => false } })
    expect(pushState(globals, null, notListed)).toBe('off')
  })
})

describe('base64UrlBytes', () => {
  it('reads base64url with no padding', () => {
    expect(Array.from(base64UrlBytes('-_8'))).toEqual([0xfb, 0xff])
    expect(Array.from(base64UrlBytes(KEY))).toHaveLength(65)
  })
})
