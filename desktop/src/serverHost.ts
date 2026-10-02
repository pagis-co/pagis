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
import { exchangeClientCredential, reusableSession, SESSION_COOKIE } from './clientSession'
import type { Dial, ExitSocket, ExitTraffic } from './exit'
import { ExitLink, openExitSocket } from './exit'
import type { CommandRunner, HostSocket } from './host'
import { EXIT_CAPABILITY, HostLink, openWebSocket, SHELL_CAPABILITY } from './host'

export interface HostLinkDeps {
  /** The origin of the server this client is the Host for. */
  url: string
  /** The cookie jar of the client's own Electron session. */
  jar: CookieReader
  /** The Client Credential of a local installation, or `null` on a server
   *  this client did not start. */
  credential: () => string | null
  /** What the machine declares. The default is the shell alone. A machine
   *  that declares the exit also opens the exit socket. */
  capabilities?: readonly string[]
  /** How the socket opens. The default is the real WebSocket. */
  open?: (url: string, secret: string) => Promise<HostSocket>
  /** How the exit socket opens. The default is the real WebSocket. */
  openExit?: (url: string, secret: string, hostId: string) => Promise<ExitSocket>
  /** How a dispatched command runs. The default is the real shell. */
  run?: CommandRunner
  /** How the exit dials a connection. The default is the dial of the Home Exit. */
  dial?: Dial
  /** What counts the exit traffic of the machine, which the tray shows. */
  traffic?: ExitTraffic
  retryMs?: number
  request?: typeof fetch
}

/** The links of this machine to one server: the Host socket, and the exit
 *  socket of a machine that declares the exit. The client starts and
 *  stops them together. */
export interface HostLinks {
  start(): void
  stop(): void
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
 *
 * The exit socket opens once the Host socket registered, with the id of
 * that registration and the Session of that socket. A Session that ended
 * on either socket is used again by neither.
 */
export function hostLinkFor(deps: HostLinkDeps): HostLinks {
  const open = deps.open ?? ((url, secret) => openWebSocket(url, secret))
  const capabilities = deps.capabilities ?? [SHELL_CAPABILITY]
  let current: string | null = null
  let ended: string | null = null
  const jar: CookieReader = {
    get: async (filter) =>
      (await deps.jar.get(filter)).filter((cookie) => cookie.value !== ended),
  }
  const host = new HostLink(
    async () => {
      current = await hostSession({ ...deps, jar })
      return open(deps.url, current)
    },
    deps.retryMs,
    deps.run,
    () => {
      ended = current
    },
    capabilities,
  )
  if (!capabilities.includes(EXIT_CAPABILITY)) return host

  const openExit = deps.openExit ?? ((url, secret, hostId) => openExitSocket(url, secret, hostId))
  let exitSession: string | null = null
  const exit = new ExitLink(
    async () => {
      const hostId = host.registeredId()
      if (hostId === null || current === null || current === ended) {
        throw new Error('the Host socket has not registered')
      }
      exitSession = current
      return openExit(deps.url, exitSession, hostId)
    },
    deps.retryMs,
    deps.dial,
    () => {
      ended = exitSession
    },
    deps.traffic,
  )
  return {
    start: () => {
      host.start()
      exit.start()
    },
    stop: () => {
      host.stop()
      exit.stop()
    },
  }
}

/**
 * Turn the Person's Home Exit off (ADR-0029): the client asks the server
 * to clear the choice of the Person whose Session it holds, as the
 * Settings card does. Their Computers then reach the internet from the
 * server, and the exit traffic of this machine stops.
 *
 * It throws with the answer of the server when the server does not turn
 * it off.
 */
export async function turnOffHomeExit(
  deps: Pick<HostLinkDeps, 'url' | 'jar' | 'credential' | 'request'>,
): Promise<void> {
  const request = deps.request ?? fetch
  const secret = await hostSession(deps)
  const response = await request(new URL('/api/v1/settings/home-exit', deps.url), {
    method: 'DELETE',
    redirect: 'error',
    headers: { cookie: `${SESSION_COOKIE}=${secret}` },
  })
  if (response.ok) return
  const detail = await response
    .json()
    .then((body: { error?: { message?: string } }) => body.error?.message)
    .catch(() => undefined)
  throw new Error(
    `the Pagis server did not turn the Home Exit off: HTTP ${response.status}${detail ? `, ${detail}` : ''}`,
  )
}
