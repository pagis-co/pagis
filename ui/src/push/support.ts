// The state of this client for Notifications (ADR-0030). The function
// reads the browser globals that it gets, so a test gives it fake ones.

/** The browser globals that decide whether this client gets a Web Push.
 *  `window` is one. */
export interface PushGlobals {
  navigator: {
    userAgent: string
    serviceWorker?: unknown
    /** Safari on iOS only: `false` in a Safari tab, `true` in a Home
     *  Screen web app. */
    standalone?: boolean
  }
  PushManager?: unknown
  Notification?: { permission: NotificationPermission }
  /** The bridge of the Mobile App. `@capacitor/core` also puts one on a
   *  page in a browser, where `isNativePlatform()` is false. */
  Capacitor?: { isNativePlatform(): boolean }
}

/** The state of this client:
 *  - `not-available`: the client has no service worker or no Push API,
 *    or it is the Client App, which Electron gives no push service;
 *  - `add-to-home-screen`: a Safari tab on iOS, where only a Home Screen
 *    web app gets a Web Push;
 *  - `blocked`: the Person denied the notification permission;
 *  - `off`: the browser holds no Push Subscription;
 *  - `on`: the browser holds a Push Subscription that the daemon lists,
 *    with the daemon's key;
 *  - `stale`: the browser holds a Push Subscription that the daemon does
 *    not list, or one with another key. The daemon's list is the truth,
 *    so the section ends it, and the state is then `off`;
 *  - `mobile-app`: the page runs in the native shell of the Mobile App. */
export type PushState =
  | 'not-available'
  | 'add-to-home-screen'
  | 'blocked'
  | 'off'
  | 'on'
  | 'stale'
  | 'mobile-app'

/** The Push Subscription that the browser holds, as far as the state
 *  reads it. */
export interface HeldSubscription {
  options: { applicationServerKey: ArrayBuffer | null }
}

/** What the daemon says: whether it lists a Push Subscription of the
 *  Session of this client, and the public half of its VAPID Key. */
export interface DaemonPush {
  listed: boolean
  key: string
}

/** Whether this client can subscribe at all, before a Push Subscription
 *  is read. `supported` means the browser has the Push API and the
 *  Person did not deny the permission. */
export function pushSupport(
  globals: PushGlobals,
): Exclude<PushState, 'off' | 'on' | 'stale'> | 'supported' {
  if (globals.Capacitor?.isNativePlatform() === true) return 'mobile-app'
  if (globals.navigator.userAgent.includes('Electron/')) return 'not-available'
  if (globals.navigator.standalone === false) return 'add-to-home-screen'
  if (!globals.navigator.serviceWorker || !globals.PushManager) return 'not-available'
  if (globals.Notification?.permission === 'denied') return 'blocked'
  return 'supported'
}

/** The state of this client, from the browser globals, the Push
 *  Subscription that the browser holds (`null` for none), and what the
 *  daemon says. */
export function pushState(
  globals: PushGlobals,
  held: HeldSubscription | null,
  daemon: DaemonPush,
): PushState {
  const support = pushSupport(globals)
  if (support !== 'supported') return support
  if (held === null) return 'off'
  const sameKey = sameBytes(held.options.applicationServerKey, base64UrlBytes(daemon.key))
  return daemon.listed && sameKey ? 'on' : 'stale'
}

/** The bytes of a base64url text, with or without padding. The VAPID
 *  Key comes as base64url with no padding. */
export function base64UrlBytes(text: string): Uint8Array<ArrayBuffer> {
  const binary = atob(text.replaceAll('-', '+').replaceAll('_', '/'))
  return Uint8Array.from(binary, (character) => character.charCodeAt(0))
}

function sameBytes(buffer: ArrayBuffer | null, bytes: Uint8Array): boolean {
  if (buffer === null || buffer.byteLength !== bytes.length) return false
  const held = new Uint8Array(buffer)
  return held.every((byte, index) => byte === bytes[index])
}
