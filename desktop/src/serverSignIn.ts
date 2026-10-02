// The sign-in of a server this client did not start.
//
// The Person signs in on the server's own page, in the product window, as
// Slack, Mattermost and Element do: the sign-in page with a password, or
// the page of a Sign-In Link, which trades the secret of the link
// (ADR-0028). The setup page asks for the address or the link alone, and
// the client never holds the password or the secret. The server's answer
// puts the Session cookie in the jar of the client's Electron session.
// This module opens the product window on that page and watches the jar.
// It tells the caller when the jar first holds a Session of the server,
// which is when the machine can register as a Host, and it holds the
// cookie as `Secure` on an `https://` origin (ADR-0024).

import {
  type CookieJar,
  type CookieReader,
  type ProductWindow,
  SESSION_COOKIE,
  holdSession,
  sessionInJar,
} from './clientSession'

/**
 * Open the product window at the origin of the server, or at a Sign-In
 * Link on that origin.
 *
 * The page of a Sign-In Link opens the app as soon as the server answers
 * the trade, and that can be before the page finished its own load.
 * Electron then rejects the load with `ERR_ABORTED`, and the window goes
 * on to the app, as the navigation rule of the window allows. Each other
 * failure of the load is a failure.
 */
export async function openServerPage(window: ProductWindow, url: string): Promise<void> {
  try {
    await window.loadURL(url)
  } catch (error) {
    if ((error as { code?: unknown } | null)?.code !== 'ERR_ABORTED') throw error
  }
}

/** A cookie as the jar reports it in a change. */
export interface ChangedCookie {
  name: string
  value: string
  secure?: boolean
  expirationDate?: number
}

type CookieChange = (event: unknown, cookie: ChangedCookie, cause: string, removed: boolean) => void

/** What the shell uses of the change events of its cookie jar. */
export interface CookieChanges {
  on(event: 'changed', listener: CookieChange): unknown
  removeListener(event: 'changed', listener: CookieChange): unknown
}

/**
 * Watch the jar for the Session of the server at `url`, until the
 * returned function stops the watch.
 *
 * `signedIn` runs once, when the jar first holds a Session of that
 * server: at once on a later start, and after the sign-in on the first
 * one. A later sign-in, after a sign-out or the end of a Session, needs
 * no new call, because the Host link reads the jar again by itself.
 *
 * On an `https://` origin, each Session cookie that the server set with
 * no `Secure` goes back into the jar as `Secure`: a proxy that does not
 * report TLS makes the server leave it out, and the cookie must still
 * never go out over `http://`.
 */
export function watchServerSignIn(
  url: string,
  jar: CookieJar & CookieReader & CookieChanges,
  signedIn: () => void,
): () => void {
  const https = new URL(url).protocol === 'https:'
  let told = false
  let stopped = false

  const check = async (): Promise<void> => {
    const held = await sessionInJar(url, jar)
    if (stopped || held === null) return
    if (https && held.secure !== true) {
      // The jar reports the Secure cookie as a change of its own, and
      // that change tells the caller.
      await holdSession(url, held, jar)
      return
    }
    if (told) return
    told = true
    signedIn()
  }
  const checkAndReport = (): void => {
    check().catch((error: unknown) => {
      console.error(`pagis: the Session of ${url} could not be read: ${String(error)}`)
    })
  }
  const listener: CookieChange = (_event, cookie, _cause, removed) => {
    if (!removed && cookie.name === SESSION_COOKIE) checkAndReport()
  }

  // The watch starts before the first read, so a sign-in between the two
  // is not lost.
  jar.on('changed', listener)
  checkAndReport()
  return () => {
    stopped = true
    jar.removeListener('changed', listener)
  }
}
