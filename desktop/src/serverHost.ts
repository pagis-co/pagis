// How this client becomes the Host of the machine it runs on.
//
// The wiring is three steps, and it is a function of its own so that a
// test reaches it without `clientMain`. Find the Session this client
// holds, open the WebSocket with it, and register the machine.
//
// Where the Session comes from differs between a local installation and a
// server, and nowhere else:
//
// - A local installation holds a Client Credential. The jar's Session is
//   used again when it is live, and the credential opens one when it is
//   not.
// - A server holds no credential. The person's own sign-in on the
//   server's page is the only Session there is, so the socket reads the
//   jar the product window signed in to. The client starts the link when
//   that sign-in puts the Session in the jar (`serverSignIn.ts`).

import type { CookieReader } from './clientSession'
import { exchangeClientCredential, reusableSession } from './clientSession'
import type { CommandRunner, HostSocket } from './host'
import { HostLink, openWebSocket } from './host'

export interface HostLinkDeps {
  /** The origin of the server this client is the Host for. */
  url: string
  /** The cookie jar of the client's own Electron session. */
  jar: CookieReader
  /** The Client Credential of a local installation, or `null` on a server
   *  this client did not start. */
  credential: () => string | null
  /** How the socket opens. The default is the real WebSocket. */
  open?: (url: string, secret: string) => Promise<HostSocket>
  /** How a dispatched command runs. The default is the real shell. */
  run?: CommandRunner
  retryMs?: number
  request?: typeof fetch
}

/**
 * The Session this client authenticates its Host socket with.
 *
 * It throws on a server whose person has not signed in yet, which the
 * link reads as "not connected" and retries.
 */
export async function hostSession(deps: HostLinkDeps): Promise<string> {
  const request = deps.request ?? fetch
  const held = await reusableSession(deps.url, deps.jar, request)
  if (held !== null) return held.secret
  const credential = deps.credential()
  if (credential !== null) {
    return (await exchangeClientCredential(deps.url, credential, undefined, request)).secret
  }
  throw new Error('nobody is signed in to the Pagis server yet')
}

/**
 * The Host link of one server, not yet started. The caller starts it and
 * stops it with the client.
 *
 * A socket that the daemon closed because its Session ended leaves that
 * Session in the jar until the page signs in again. The link reads the
 * jar without that Session, so it opens no socket with it and does not
 * ask the server about it again. On a server it waits for the Person to
 * sign in again; on a local installation it trades the Client Credential
 * for a new Session.
 */
export function hostLinkFor(deps: HostLinkDeps): HostLink {
  const open = deps.open ?? ((url, secret) => openWebSocket(url, secret))
  let current: string | null = null
  let ended: string | null = null
  const jar: CookieReader = {
    get: async (filter) =>
      (await deps.jar.get(filter)).filter((cookie) => cookie.value !== ended),
  }
  return new HostLink(
    async () => {
      current = await hostSession({ ...deps, jar })
      return open(deps.url, current)
    },
    deps.retryMs,
    deps.run,
    () => {
      ended = current
    },
  )
}
