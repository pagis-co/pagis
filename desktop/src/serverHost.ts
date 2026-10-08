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
//
// Beside the Host socket, the client opens the session socket, which
// carries the Coding Sessions of the machine, and the exit socket of a
// machine that declares the exit.

import type { ByteSocket } from './byteSocket'
import type { CookieReader } from './clientSession'
import { exchangeClientCredential, reusableSession, SESSION_COOKIE } from './clientSession'
import type { Dial, ExitTraffic } from './exit'
import { ExitLink, openExitSocket } from './exit'
import type { CommandRunner, HostSocket } from './host'
import {
  EXIT_CAPABILITY,
  HARNESS_CAPABILITY_PREFIX,
  harnessesOnPath,
  HostLink,
  openWebSocket,
  SHELL_CAPABILITY,
} from './host'
import { openSessionSocket, SessionLink } from './sessions'

export interface HostLinkDeps {
  /** The origin of the server this client is the Host for. */
  url: string
  /** The cookie jar of the client's own Electron session. */
  jar: CookieReader
  /** The Client Credential of a local installation, or `null` on a server
   *  this client did not start. */
  credential: () => string | null
  /** What the machine declares besides its Coding Harnesses. The default
   *  is the shell alone. A machine that declares the exit also opens the
   *  exit socket. */
  capabilities?: readonly string[]
  /** How the socket opens. The default is the real WebSocket. */
  open?: (url: string, secret: string) => Promise<HostSocket>
  /** How the exit socket opens. The default is the real WebSocket. */
  openExit?: (url: string, secret: string, hostId: string) => Promise<ByteSocket>
  /** How the session socket opens. The default is the real WebSocket. */
  openSessions?: (url: string, secret: string, hostId: string) => Promise<ByteSocket>
  /** How the client reads the login-shell environment, for the search of
   *  the Coding Harnesses and for each Coding Session. The default runs
   *  the person's login shell. */
  environment?: () => Promise<Record<string, string>>
  /** How a dispatched command runs. The default is the real shell. */
  run?: CommandRunner
  /** How the exit dials a connection. The default is the dial of the Home Exit. */
  dial?: Dial
  /** What counts the exit traffic of the machine, which the tray shows. */
  traffic?: ExitTraffic
  retryMs?: number
  request?: typeof fetch
}

/** The links of this machine to one server: the Host socket, the session
 *  socket, and the exit socket of a machine that declares the exit. The
 *  client starts and stops them together. */
export interface HostLinks {
  start(): void
  /** Close every socket and kill every process of a Coding Session. It
   *  resolves when each process exited. */
  stop(): Promise<void>
  /** The processes of Coding Sessions that run now. */
  codingSessions(): number
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
 * The Host socket declares each Coding Harness of the catalog whose
 * programs are on the PATH of the login shell, with a second
 * registration. The session socket and the exit socket open once the Host
 * socket registered, with the id of that registration and the Session of
 * that socket. The session socket opens once the daemon acknowledged a
 * registration that holds a `harness:` capability, on a Local
 * Installation too, because the daemon is never a Host (ADR-0015). The
 * exit socket opens only on a machine that declares the exit. A Session
 * that ended on any socket is used again by none.
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
    (catalog) => harnessesOnPath(catalog, deps.environment),
  )
  /** The host id and the Session of the registered Host socket. */
  const registration = (): { hostId: string; secret: string } => {
    const hostId = host.registeredId()
    if (hostId === null || current === null || current === ended) {
      throw new Error('the Host socket has not registered')
    }
    return { hostId, secret: current }
  }

  const openSessions = deps.openSessions ?? openSessionSocket
  let sessionsSession: string | null = null
  const sessions = new SessionLink(
    async () => {
      const { hostId, secret } = registration()
      if (!host.registeredCapabilities().some((held) => held.startsWith(HARNESS_CAPABILITY_PREFIX))) {
        throw new Error('the registration of the Host holds no Coding Harness')
      }
      sessionsSession = secret
      return openSessions(deps.url, secret, hostId)
    },
    {
      send: (frame) => {
        // With no Host socket open the exit goes nowhere: the daemon
        // lost the session socket of the process too.
        host.send(JSON.stringify(frame))
      },
      environment: deps.environment,
    },
    deps.retryMs,
    () => {
      ended = sessionsSession
    },
  )

  let exit: ExitLink | null = null
  if (capabilities.includes(EXIT_CAPABILITY)) {
    const openExit = deps.openExit ?? openExitSocket
    let exitSession: string | null = null
    exit = new ExitLink(
      async () => {
        const { hostId, secret } = registration()
        exitSession = secret
        return openExit(deps.url, secret, hostId)
      },
      deps.retryMs,
      deps.dial,
      () => {
        ended = exitSession
      },
      deps.traffic,
    )
  }

  return {
    start: () => {
      host.start()
      sessions.start()
      exit?.start()
    },
    stop: async () => {
      host.stop()
      exit?.stop()
      await sessions.stop()
    },
    codingSessions: () => sessions.running,
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
