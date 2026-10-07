/**
 * What the Person types in the Server address field of the Connect
 * screen: the address of a server, or a Sign-In Link of one,
 * `https://<host>/sign-in#<secret>` (ADR-0028).
 *
 * The rules are the rules of the Client App (`desktop/src/origin.ts`),
 * with one change: `http://` on a loopback host passes in a debug build
 * only. A phone in the hands of a Person has no Pagis server on its own
 * loopback, and a developer reaches a daemon on the computer of the
 * simulator, or one that `adb reverse` forwards to the emulator.
 */

/** Which build of the app reads the field. */
export interface BuildType {
  /** A debug build, which takes `http://` on a loopback host. */
  debug: boolean
}

/** The hosts that name this machine and no other. The W3C Secure
 *  Contexts specification trusts an origin on them without TLS. */
const LOOPBACK_HOSTS = new Set(['127.0.0.1', '[::1]', 'localhost'])

/** The path of the page that a Sign-In Link opens (ADR-0028). */
const SIGN_IN_PATH = '/sign-in'

/** What the Person typed, read. */
export interface ServerAddress {
  /** The origin of the server. The app keeps it, and nothing more. */
  origin: string
  /** Where the web view opens: the Sign-In Link on that origin, or else
   *  the origin. The page of the Product App at `/sign-in` trades the
   *  secret for a Session. Nothing keeps this address, because a link
   *  holds a secret. */
  opens: string
}

/**
 * Whether the app trusts a server at this origin: an `https://` origin,
 * which TLS authenticates, or in a debug build an `http://` origin on a
 * loopback host.
 */
export function isTrustedServerOrigin(url: string | URL, build: BuildType): boolean {
  let parsed: URL
  try {
    parsed = new URL(url)
  } catch {
    return false
  }
  if (parsed.protocol === 'https:') return true
  return build.debug && parsed.protocol === 'http:' && LOOPBACK_HOSTS.has(parsed.hostname)
}

/**
 * Read the Server address field.
 *
 * A bare name means TLS, as in the address bar of a browser. A path, a
 * query and a fragment of an address are dropped: an origin is the
 * scheme, the host and the port. The link that the web view opens is
 * made again from the checked origin and the secret, so the web view
 * opens no other server and no other page. A link with no secret is
 * refused: its page would sign nobody in.
 */
export function serverAddress(typed: string, build: BuildType): ServerAddress {
  const url = serverUrl(typed, build)
  const origin = `${url.protocol}//${url.host}/`
  if (url.pathname !== SIGN_IN_PATH) return { origin, opens: origin }
  if (url.hash.length <= 1) {
    throw new Error('This sign-in link is not complete. Copy the whole link, then paste it again.')
  }
  return { origin, opens: `${url.protocol}//${url.host}${SIGN_IN_PATH}${url.hash}` }
}

/**
 * Read the text of a scanned QR code. Only a Sign-In Link signs the app
 * in from a scan: an address or other text holds no secret. A link that
 * the field refuses gets the words of the field.
 */
export function scannedSignInLink(scanned: string, build: BuildType): ServerAddress {
  const noLink =
    'This QR code holds no sign-in link of a Pagis server. Scan the QR code in Settings → Sessions ' +
    'on a browser or app that is signed in.'
  if (scanned.trim().length === 0) throw new Error(noLink)
  const address = serverAddress(scanned, build)
  if (address.opens === address.origin) throw new Error(noLink)
  return address
}

/** What the Person typed, as a URL of a server that the app trusts, or
 *  an error in words for the Person. */
function serverUrl(typed: string, build: BuildType): URL {
  const raw = typed.trim()
  if (raw.length === 0) throw new Error('Enter the address of your Pagis server, or paste a sign-in link.')
  const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(raw) ? raw : `https://${raw}`
  let url: URL
  try {
    url = new URL(withScheme)
  } catch {
    throw new Error(`${raw} is not a server address.`)
  }
  if (url.protocol !== 'https:' && url.protocol !== 'http:') {
    throw new Error('A Pagis server address starts with https:// or http://.')
  }
  if (url.hostname.length === 0) throw new Error(`${raw} names no server.`)
  // A name and a password in an address are not a sign-in. The Person
  // signs in on the page of the server.
  if (url.username.length > 0 || url.password.length > 0) {
    throw new Error('Leave the user name and the password out of the address. You sign in on the page of the server.')
  }
  if (!isTrustedServerOrigin(url, build)) {
    throw new Error(
      'Pagis connects to a server only over https://. ' +
        'Turn on Remote Access on the computer of the server, as docs.pagis.co/client-app/several-people ' +
        'shows, or put a proxy that holds TLS in front of the server. Then type its https:// address.',
    )
  }
  return url
}
