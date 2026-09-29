/**
 * Where the client talks to a server.
 *
 * An installation the client starts itself answers on loopback, and that
 * is the one place the address is written down. An installation the
 * client did not start answers at the origin the person named, which is
 * stored with the connection: no other module holds a literal address.
 *
 * One rule, `isTrustedServerOrigin`, decides which server origin the
 * client trusts. The address the person types, the stored connection
 * and the Host socket all use it. `opensInSystemBrowser` decides which
 * address a page of the client opens in the system browser.
 */

/** The interface a local Server Runtime binds (ADR-0025). */
export const LOOPBACK_HOST = '127.0.0.1'

/** The hosts that name this machine and no other. The W3C Secure
 *  Contexts specification trusts an origin on them without TLS. */
const LOOPBACK_HOSTS = new Set([LOOPBACK_HOST, '[::1]', 'localhost'])

/** The origin of a local Server Runtime on a port. */
export function loopbackOrigin(port: number): string {
  return `http://${LOOPBACK_HOST}:${port}/`
}

/**
 * Whether the client trusts a server at this origin: an `https://`
 * origin, which TLS authenticates, or an `http://` origin on a loopback
 * host, such as the server this client started or an SSH tunnel.
 *
 * The Person signs in to that server in the product window, the client
 * holds the Session, and it runs the commands that the server
 * dispatches to the Host. Over `http://` to
 * another computer, all of them cross the network as clear text, and
 * anybody on the network path can read them or send commands of their
 * own. Docker Engine deprecates its API on a non-loopback TCP address
 * without TLS for the same reason: that socket runs commands.
 */
export function isTrustedServerOrigin(url: string | URL): boolean {
  let parsed: URL
  try {
    parsed = new URL(url)
  } catch {
    return false
  }
  if (parsed.protocol === 'https:') return true
  return parsed.protocol === 'http:' && LOOPBACK_HOSTS.has(parsed.hostname)
}

/**
 * The origin of the server the person named, from whatever they typed.
 *
 * A bare name means TLS, as it does in a browser's address bar, because
 * a server the client did not start is trusted through TLS and the
 * person's sign-in. An `http://` address is refused unless its host is
 * loopback. A path, a query and a fragment are dropped: an origin is the
 * scheme, the host and the port and nothing after them.
 */
export function serverOrigin(typed: string): string {
  const raw = typed.trim()
  if (raw.length === 0) throw new Error('Enter the address of your Pagis server.')
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
  // A name and a password in an address are not a sign-in, and the
  // client would carry them into every call it makes. The Person signs
  // in on the server's own page.
  if (url.username.length > 0 || url.password.length > 0) {
    throw new Error('Leave the user name and the password out of the address. You sign in on the page of the server.')
  }
  if (!isTrustedServerOrigin(url)) {
    throw new Error(
      'Pagis connects to a server on another computer only over https://. ' +
      'Put TLS in front of the server with Caddy, Tailscale Serve or Cloudflare Tunnel, ' +
      'as docs/DEPLOYING-A-SERVER.md shows, then type its https:// address.',
    )
  }
  return `${url.protocol}//${url.host}/`
}

/**
 * Whether the Client App opens an address that a page asks for in the
 * system browser.
 *
 * An `https:` address and a `mailto:` address open there. An `http:`
 * address opens only when it names the server that the product window
 * shows: a loopback host on the port of that server, when the product
 * window itself shows a loopback `http:` origin. That is the Public
 * Origin of a Local Installation, `http://127.0.0.1:<port>` or
 * `http://localhost:<port>`, where the start route of a Google
 * authorization is. The Person consents at Google in that browser.
 * Any other `http:` address stays closed, because nothing authenticates
 * its server: another program on this machine, or another computer.
 */
export function opensInSystemBrowser(target: string, productOrigin: string | null): boolean {
  let url: URL
  try {
    url = new URL(target)
  } catch {
    return false
  }
  if (url.protocol === 'https:' || url.protocol === 'mailto:') return true
  if (url.protocol !== 'http:' || productOrigin === null) return false
  let product: URL
  try {
    product = new URL(productOrigin)
  } catch {
    return false
  }
  return (
    product.protocol === 'http:' &&
    LOOPBACK_HOSTS.has(product.hostname) &&
    LOOPBACK_HOSTS.has(url.hostname) &&
    url.port === product.port
  )
}
