import * as os from 'node:os'

/** The cookie the daemon sets for a Session. */
export const SESSION_COOKIE = 'pagis_session'

/** A Session this client holds: the cookie value, and when the cookie
 *  ends, in Unix seconds, as the server's `Max-Age` says. A cookie with
 *  no end is gone when the app quits, so a Session the server gave an
 *  end keeps it in the jar. `secure` is true when the server set the
 *  `Secure` attribute, which it does over TLS, so the jar sends the
 *  cookie over TLS only. */
export interface HeldSession {
  secret: string
  expiresAt?: number
  secure?: boolean
}

/** What the shell writes to the Electron session's cookie jar. A cookie
 *  with no `expirationDate` is a session cookie, which Electron drops
 *  when the app quits. */
export interface CookieJar {
  set(cookie: {
    url: string
    name: string
    value: string
    httpOnly?: boolean
    sameSite?: 'strict'
    secure?: boolean
    expirationDate?: number
  }): Promise<void>
}

/** What the shell reads back out of that jar. The Session of a server
 *  the client did not start is set by the page the person signed in on,
 *  so the jar is where the main process finds it. */
export interface CookieReader {
  get(filter: { url: string; name: string }): Promise<{ value: string; expirationDate?: number; secure?: boolean }[]>
}

/** What the shell uses of the window that shows the product. */
export interface ProductWindow {
  loadURL(url: string): Promise<void>
}

/**
 * Exchange the Client Credential of this installation for a Session.
 * The daemon answers with the session cookie; the value of that
 * cookie is the Session the caller authenticates with.
 */
export async function exchangeClientCredential(
  url: string,
  credential: string,
  signal?: AbortSignal,
  request: typeof fetch = fetch,
): Promise<HeldSession> {
  const response = await request(new URL('/api/v1/sessions/client', url), {
    method: 'POST',
    redirect: 'error',
    signal,
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ credential, client_name: os.hostname() }),
  })
  if (!response.ok) {
    throw new Error(`the Pagis server refused the client credential with HTTP ${response.status}`)
  }
  const held = heldSession(response.headers.getSetCookie())
  if (!held) {
    throw new Error('the Pagis server returned no session cookie')
  }
  return held
}

/** The Session the jar holds for an origin, or null when the person has
 *  not signed in there yet. */
export async function sessionInJar(url: string, jar: CookieReader): Promise<HeldSession | null> {
  const cookie = (await jar.get({ url, name: SESSION_COOKIE }))[0]
  if (cookie === undefined) return null
  return {
    secret: cookie.value,
    ...(cookie.expirationDate === undefined ? {} : { expiresAt: cookie.expirationDate }),
    ...(cookie.secure === true ? { secure: true } : {}),
  }
}

/**
 * The Session this client may use again, or null.
 *
 * A launch opened a Session for each of its surfaces — the product
 * window, the Host socket, and the administration window — and each row
 * lives thirty days, so a client that runs every day left a table of dead
 * rows behind it. The cookie the jar already holds is a Session of this
 * client, so the client offers it to the daemon first: the person's own
 * read answering `200` means the Session is live, and nothing is traded.
 * Anything else, including a refusal and a daemon that does not answer,
 * means the cookie is spent or of another installation, and the caller
 * trades the Client Credential for a fresh Session.
 */
export async function reusableSession(
  url: string,
  jar: CookieReader,
  request: typeof fetch = fetch,
  signal?: AbortSignal,
): Promise<HeldSession | null> {
  const held = await sessionInJar(url, jar)
  if (held === null) return null
  try {
    const response = await request(new URL('/api/v1/user', url), {
      redirect: 'error',
      signal,
      headers: { cookie: `${SESSION_COOKIE}=${held.secret}` },
    })
    return response.ok ? held : null
  } catch {
    return null
  }
}

/**
 * Show the product to the person, already signed in. The page reads the
 * cookie from the jar of its Electron session, so the Session must be in
 * that jar before the window loads the URL.
 */
export async function openSignedIn(
  url: string,
  credential: string,
  jar: CookieJar & CookieReader,
  window: ProductWindow,
  request: typeof fetch = fetch,
): Promise<void> {
  await openSignedInAt(url, url, credential, jar, window, request)
}

/**
 * Show a page of this installation, already signed in, where the page is
 * not on the port that answers the Client Credential.
 *
 * The Administration Interface is a second listener of the same process
 * and it holds no credential exchange of its own, so the Session is
 * traded on the product port and set for the administration one. A
 * cookie is host-only and a browser tells no two ports of one host
 * apart, so the one Session serves both pages.
 */
export async function openSignedInAt(
  url: string,
  exchangeUrl: string,
  credential: string,
  jar: CookieJar & CookieReader,
  window: ProductWindow,
  request: typeof fetch = fetch,
): Promise<void> {
  // The Session the jar already holds serves this page too: a cookie is
  // host-only, so the product port and the administration port read the
  // same one. Only a jar with no live Session trades the credential.
  const held =
    (await reusableSession(exchangeUrl, jar, request)) ??
    (await exchangeClientCredential(exchangeUrl, credential, undefined, request))
  await openWithSession(url, held, jar, window)
}

/** Put a Session in the jar, and load the page. */
export async function openWithSession(
  url: string,
  held: HeldSession,
  jar: CookieJar,
  window: ProductWindow,
): Promise<void> {
  await holdSession(url, held, jar)
  await window.loadURL(url)
}

/**
 * Put a Session in the jar of the client's Electron session.
 *
 * The cookie carries the Session's end, so the jar keeps it across a
 * quit until the Session ends. It is `Secure` where the server set that,
 * and on every `https://` origin: a proxy that does not report TLS makes
 * the server set no `Secure`, and the cookie must still never go out
 * over `http://`. A loopback `http://` origin gets no `Secure`.
 */
export async function holdSession(url: string, held: HeldSession, jar: CookieJar): Promise<void> {
  const secure = held.secure === true || new URL(url).protocol === 'https:'
  await jar.set({
    url,
    name: SESSION_COOKIE,
    value: held.secret,
    httpOnly: true,
    sameSite: 'strict',
    ...(secure ? { secure: true } : {}),
    ...(held.expiresAt === undefined ? {} : { expirationDate: held.expiresAt }),
  })
}

/** The Session a `Set-Cookie` answer carries, with the end its
 *  `Max-Age` gives, counted from `now` in milliseconds, and the `Secure`
 *  attribute where the server set it. */
function heldSession(headers: string[], now: number = Date.now()): HeldSession | null {
  for (const header of headers) {
    const secret = /^\s*pagis_session=([^;]*)/.exec(header)?.[1]
    if (!secret) continue
    const maxAge = /;\s*max-age=(\d+)/i.exec(header)?.[1]
    const secure = /;\s*secure\s*(?:;|$)/i.test(header)
    return {
      secret,
      ...(maxAge === undefined ? {} : { expiresAt: Math.floor(now / 1000) + Number(maxAge) }),
      ...(secure ? { secure: true } : {}),
    }
  }
  return null
}
