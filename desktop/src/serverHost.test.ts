// The Host wiring of a server this client did not start.
//
// These tests prove that a connected client registers its machine with
// the Session the person's own sign-in left in the jar, against a socket
// a test drives. The last case is a Local Installation: the loopback
// origin passes the origin rule, and the real `openWebSocket` opens the
// Host socket there.

import { EventEmitter } from 'node:events'
import type { Socket } from 'node:net'
import { duplexPair } from 'node:stream'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { openSignedIn } from './clientSession'
import { type ExitSocket, ExitTraffic } from './exit'
import { EXIT_CAPABILITY, type HostSocket, SHELL_CAPABILITY } from './host'
import { loopbackOrigin } from './origin'
import { hostLinkFor, hostSession, turnOffHomeExit } from './serverHost'
import { watchServerSignIn } from './serverSignIn'

const SERVER = 'https://pagis.example.com/'

/** A socket a test drives: it keeps what the client sent. */
class FakeSocket implements HostSocket {
  sent: Record<string, unknown>[] = []

  send(frame: string): void {
    this.sent.push(JSON.parse(frame) as Record<string, unknown>)
  }

  onMessage(): void {}
  onClose(): void {}
  close(): void {}
}

/** An answer to the person's own read, which is how the client checks
 *  that a Session in the jar is still live. */
function live(): Response {
  return new Response(JSON.stringify({ id: 'user-1' }), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  })
}

/** Wait until the link has opened its socket. `start` is not awaited, so
 *  the first connect lands on the next turns of the loop. */
async function settle(): Promise<void> {
  for (let turn = 0; turn < 20; turn += 1) {
    await Promise.resolve()
    await new Promise((resolve) => setTimeout(resolve, 0))
  }
}

describe('the Host of a server this client did not start', () => {
  it('registers the machine with the Session in the jar, after the person signed in', async () => {
    const socket = new FakeSocket()
    const open = vi.fn(async (_url: string, _secret: string) => socket as HostSocket)
    const link = hostLinkFor({
      url: SERVER,
      jar: { get: async () => [{ value: 'the-persons-session' }] },
      // A server holds no Client Credential.
      credential: () => null,
      open,
      request: async () => live(),
    })

    link.start()
    await settle()
    link.stop()

    // The socket opened on the server's own origin, with the Session the
    // sign-in left in the jar and no credential trade.
    expect(open).toHaveBeenCalledWith(SERVER, 'the-persons-session')
    // The first frame of any socket is the auth frame; the second
    // is what makes this client a Host.
    expect(socket.sent[0]).toMatchObject({ type: 'auth' })
    expect(socket.sent[1]).toMatchObject({
      type: 'register_host',
      capabilities: ['shell'],
    })
    expect(typeof socket.sent[1]?.name).toBe('string')
    expect(typeof socket.sent[1]?.platform).toBe('string')
  })

  it('opens nothing until the person signs in', async () => {
    const open = vi.fn(async () => new FakeSocket() as HostSocket)
    const link = hostLinkFor({
      url: SERVER,
      // Nobody has signed in, so the jar holds no Session.
      jar: { get: async () => [] },
      credential: () => null,
      open,
      retryMs: 60_000,
      request: async () => live(),
    })

    link.start()
    await settle()
    link.stop()

    expect(open).not.toHaveBeenCalled()
  })

  /** The client wiring: the link starts when the Person signs in on the
   *  server's own page, which puts the Session in the jar. */
  it('registers the machine only after the Person signs in on the page of the server', async () => {
    let session: string | null = null
    const jar = Object.assign(new EventEmitter(), {
      get: async () => (session === null ? [] : [{ value: session, secure: true }]),
      set: async () => {},
    })
    const open = vi.fn(async (_url: string, _secret: string) => new FakeSocket() as HostSocket)
    const link = hostLinkFor({ url: SERVER, jar, credential: () => null, open, request: async () => live() })
    const stop = watchServerSignIn(SERVER, jar, () => link.start())

    await settle()
    expect(open).not.toHaveBeenCalled()

    session = 'adas-session'
    jar.emit('changed', {}, { name: 'pagis_session', value: session, secure: true }, 'explicit', false)
    await vi.waitFor(() => expect(open).toHaveBeenCalledWith(SERVER, 'adas-session'))
    stop()
    link.stop()
  })

  it('trades the Client Credential on a local installation with an empty jar', async () => {
    const request = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => {
      expect(String(input)).toBe('https://pagis.example.com/api/v1/sessions/client')
      return new Response(JSON.stringify({ id: 'user-1' }), {
        status: 200,
        headers: {
          'content-type': 'application/json',
          'set-cookie': 'pagis_session=traded; Path=/; HttpOnly; SameSite=Strict',
        },
      })
    })

    const secret = await hostSession({
      url: SERVER,
      jar: { get: async () => [] },
      credential: () => 'a'.repeat(64),
      request,
    })

    expect(secret).toBe('traded')
  })

  it('uses the Session in the jar rather than trading a second one', async () => {
    const request = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => {
      expect(String(input)).toBe('https://pagis.example.com/api/v1/user')
      return live()
    })

    const secret = await hostSession({
      url: SERVER,
      jar: { get: async () => [{ value: 'the-live-session' }] },
      credential: () => 'a'.repeat(64),
      request,
    })

    expect(secret).toBe('the-live-session')
    expect(request).toHaveBeenCalledTimes(1)
  })
})

/** A socket the daemon closes with a code, as it does when the Session
 *  that opened the socket ends. */
class ClosingSocket implements HostSocket {
  private closeListener: ((code: number) => void) | null = null

  send(): void {}
  onMessage(): void {}
  onClose(listener: (code: number) => void): void {
    this.closeListener = listener
  }
  close(): void {}

  closeWith(code: number): void {
    this.closeListener?.(code)
  }
}

/** The daemon closes the Host socket with 1008 when its Session ends: a
 *  sign-out, an Administrator who ends every Session of the Person, or
 *  the expiry. The Session is gone, so the client does not open a socket
 *  with it again or ask the server about it again: it waits for the
 *  Person to sign in again, and registers with the new Session. */
describe('a Host socket whose Session ended', () => {
  it('opens no socket with that Session again, and registers after a new sign-in', async () => {
    let inJar = 'the-persons-session'
    const opened: string[] = []
    const sockets: ClosingSocket[] = []
    const request = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => live())
    const link = hostLinkFor({
      url: SERVER,
      jar: { get: async () => [{ value: inJar }] },
      credential: () => null,
      open: async (_url, secret) => {
        opened.push(secret)
        const socket = new ClosingSocket()
        sockets.push(socket)
        return socket
      },
      retryMs: 1,
      request,
    })

    link.start()
    await vi.waitFor(() => expect(opened).toEqual(['the-persons-session']))
    sockets[0].closeWith(1008)
    // Many retries pass. None of them opens a socket, and none of them
    // asks the server about the Session that ended.
    await new Promise((resolve) => setTimeout(resolve, 50))
    expect(opened).toEqual(['the-persons-session'])
    expect(request).toHaveBeenCalledTimes(1)

    inJar = 'a-new-session'
    await vi.waitFor(() => expect(opened).toEqual(['the-persons-session', 'a-new-session']))
    link.stop()
  })

  it('registers again at once after a close that did not end the Session', async () => {
    const opened: string[] = []
    const sockets: ClosingSocket[] = []
    const link = hostLinkFor({
      url: SERVER,
      jar: { get: async () => [{ value: 'the-persons-session' }] },
      credential: () => null,
      open: async (_url, secret) => {
        opened.push(secret)
        const socket = new ClosingSocket()
        sockets.push(socket)
        return socket
      },
      retryMs: 1,
      request: async () => live(),
    })

    link.start()
    await vi.waitFor(() => expect(opened).toHaveLength(1))
    // The daemon restarted: the connection dropped with no close frame.
    sockets[0].closeWith(1006)
    await vi.waitFor(() => expect(opened).toEqual(['the-persons-session', 'the-persons-session']))
    link.stop()
  })
})

/** A stand-in for the global WebSocket: it records its target, the
 *  handshake headers and every frame, and opens at once. */
class RecordedWebSocket extends EventTarget {
  static opened: RecordedWebSocket[] = []
  readonly target: string
  readonly headers: unknown
  sent: Record<string, unknown>[] = []

  constructor(target: URL | string, options?: { headers?: unknown }) {
    super()
    this.target = String(target)
    this.headers = options?.headers
    RecordedWebSocket.opened.push(this)
    queueMicrotask(() => this.dispatchEvent(new Event('open')))
  }

  send(frame: string): void {
    this.sent.push(JSON.parse(frame) as Record<string, unknown>)
  }

  close(): void {}
}

/** The origin rule refuses http:// to another computer. The loopback
 *  origin of the server this client started stays usable. */
describe('a Local Installation with Remote Access off', () => {
  afterEach(() => {
    RecordedWebSocket.opened = []
    vi.unstubAllGlobals()
  })

  it('opens signed in on the loopback origin and registers its Host', async () => {
    vi.stubGlobal('WebSocket', RecordedWebSocket)
    const origin = loopbackOrigin(4400)
    const credential = 'a'.repeat(64)
    // The daemon trades the Client Credential for a Session, and knows
    // the person by that Session afterwards.
    const daemon = async (input: RequestInfo | URL, init?: RequestInit) => {
      const route = new URL(String(input)).pathname
      if (route === '/api/v1/sessions/client') {
        return new Response(JSON.stringify({ id: 'user-1' }), {
          status: 200,
          headers: {
            'content-type': 'application/json',
            'set-cookie': 'pagis_session=local-session; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000',
          },
        })
      }
      const cookie = (init?.headers as Record<string, string> | undefined)?.cookie
      return cookie === 'pagis_session=local-session' ? live() : new Response(null, { status: 401 })
    }
    const cookies = new Map<string, string>()
    const jar = {
      set: vi.fn(async (cookie: { name: string; value: string }) => {
        cookies.set(cookie.name, cookie.value)
      }),
      get: async (filter: { name: string }) => {
        const value = cookies.get(filter.name)
        return value === undefined ? [] : [{ value }]
      },
    }
    const window = { loadURL: vi.fn(async () => {}) }

    await openSignedIn(origin, credential, jar, window, daemon)
    const link = hostLinkFor({ url: origin, jar, credential: () => credential, request: daemon })
    link.start()
    await settle()
    link.stop()

    expect(window.loadURL).toHaveBeenCalledWith('http://127.0.0.1:4400/')
    expect(jar.set).toHaveBeenCalledWith(expect.objectContaining({
      url: 'http://127.0.0.1:4400/',
      value: 'local-session',
    }))
    expect(RecordedWebSocket.opened).toHaveLength(1)
    const [socket] = RecordedWebSocket.opened
    expect(socket.target).toBe('ws://127.0.0.1:4400/api/v1/ws')
    expect(socket.headers).toEqual({ cookie: 'pagis_session=local-session' })
    expect(socket.sent[0]).toMatchObject({ type: 'auth' })
    expect(socket.sent[1]).toMatchObject({ type: 'register_host', capabilities: ['shell'] })
  })
})

/** A Host socket of a daemon that registers the machine under an id, as
 *  the daemon answers `register_host`. */
class RegisteringSocket implements HostSocket {
  readonly hostId: string
  sent: Record<string, unknown>[] = []
  closed = false
  private listener: ((frame: string) => void) | null = null
  private closeListener: ((code: number) => void) | null = null

  constructor(hostId: string) {
    this.hostId = hostId
  }

  send(frame: string): void {
    const parsed = JSON.parse(frame) as Record<string, unknown>
    this.sent.push(parsed)
    if (parsed.type === 'register_host') {
      queueMicrotask(() =>
        this.listener?.(JSON.stringify({ type: 'host.registered', payload: { host_id: this.hostId } })),
      )
    }
  }

  onMessage(listener: (frame: string) => void): void {
    this.listener = listener
  }

  onClose(listener: (code: number) => void): void {
    this.closeListener = listener
  }

  close(): void {
    this.closed = true
  }

  closeWith(code: number): void {
    this.closeListener?.(code)
  }
}

/** An exit socket that the daemon can close with a code, and send bytes
 *  on. */
class FakeExitSocket implements ExitSocket {
  closed = false
  private readonly closeListeners: Array<(code: number) => void> = []
  private readonly messageListeners: Array<(bytes: Uint8Array) => void> = []

  send(): void {}

  onMessage(listener: (bytes: Uint8Array) => void): void {
    this.messageListeners.push(listener)
  }

  /** Whether the link reads the socket. */
  get read(): boolean {
    return this.messageListeners.length > 0
  }

  deliver(bytes: Buffer): void {
    for (const listener of this.messageListeners) listener(bytes)
  }

  onClose(listener: (code: number) => void): void {
    this.closeListeners.push(listener)
  }

  close(): void {
    this.closed = true
  }

  closeWith(code: number): void {
    for (const listener of this.closeListeners) listener(code)
  }
}

/** A client connected to a server can be the Home Exit of its Person: it
 *  declares the exit, and opens the exit socket once its Host socket
 *  registered, with the id of that registration and the same Session. A
 *  Local Installation runs its Computers on its own machine, so its
 *  client declares the shell alone and opens no exit socket. */
describe('the exit socket of a Host', () => {
  it('opens with the registered host id and the Session of the Host socket, when the client declares the exit', async () => {
    const hostSocket = new RegisteringSocket('host-1')
    const openExit = vi.fn(async (_url: string, _secret: string, _hostId: string) => new FakeExitSocket() as ExitSocket)
    const links = hostLinkFor({
      url: SERVER,
      jar: { get: async () => [{ value: 'the-persons-session' }] },
      credential: () => null,
      open: async () => hostSocket,
      openExit,
      capabilities: [SHELL_CAPABILITY, EXIT_CAPABILITY],
      retryMs: 1,
      request: async () => live(),
    })

    links.start()
    await vi.waitFor(() => expect(openExit).toHaveBeenCalled())
    links.stop()

    expect(hostSocket.sent[1]).toMatchObject({ type: 'register_host', capabilities: ['shell', 'exit'] })
    expect(openExit).toHaveBeenCalledTimes(1)
    expect(openExit).toHaveBeenCalledWith(SERVER, 'the-persons-session', 'host-1')
  })

  it('opens no exit socket while the Host socket has not registered', async () => {
    const openExit = vi.fn(async () => new FakeExitSocket() as ExitSocket)
    const links = hostLinkFor({
      url: SERVER,
      jar: { get: async () => [{ value: 'the-persons-session' }] },
      credential: () => null,
      // The daemon never answers the registration.
      open: async () => new FakeSocket(),
      openExit,
      capabilities: [SHELL_CAPABILITY, EXIT_CAPABILITY],
      retryMs: 1,
      request: async () => live(),
    })

    links.start()
    await new Promise((resolve) => setTimeout(resolve, 30))
    links.stop()

    expect(openExit).not.toHaveBeenCalled()
  })

  it('declares the shell alone and opens no exit socket on a Local Installation', async () => {
    const hostSocket = new RegisteringSocket('host-1')
    const openExit = vi.fn(async () => new FakeExitSocket() as ExitSocket)
    const links = hostLinkFor({
      url: loopbackOrigin(4400),
      jar: { get: async () => [{ value: 'local-session' }] },
      credential: () => 'a'.repeat(64),
      open: async () => hostSocket,
      openExit,
      retryMs: 1,
      request: async () => live(),
    })

    links.start()
    await new Promise((resolve) => setTimeout(resolve, 30))
    links.stop()

    expect(hostSocket.sent[1]).toMatchObject({ type: 'register_host', capabilities: ['shell'] })
    expect(openExit).not.toHaveBeenCalled()
  })

  it('closes the Host socket and the exit socket with one stop', async () => {
    const hostSocket = new RegisteringSocket('host-1')
    const exitSocket = new FakeExitSocket()
    const openExit = vi.fn(async () => exitSocket as ExitSocket)
    const links = hostLinkFor({
      url: SERVER,
      jar: { get: async () => [{ value: 'the-persons-session' }] },
      credential: () => null,
      open: async () => hostSocket,
      openExit,
      capabilities: [SHELL_CAPABILITY, EXIT_CAPABILITY],
      retryMs: 1,
      request: async () => live(),
    })
    links.start()
    await vi.waitFor(() => expect(openExit).toHaveBeenCalled())
    // The link takes the socket on the next turn of the loop.
    await new Promise((resolve) => setTimeout(resolve, 0))

    links.stop()

    expect(hostSocket.closed).toBe(true)
    expect(exitSocket.closed).toBe(true)
  })

  /** The tray of the client shows what the exit socket carries. */
  it('counts what its streams carry in the traffic that the client shows', async () => {
    const exitSocket = new FakeExitSocket()
    const [site, siteEnd] = duplexPair()
    const traffic = new ExitTraffic()
    const links = hostLinkFor({
      url: SERVER,
      jar: { get: async () => [{ value: 'the-persons-session' }] },
      credential: () => null,
      open: async () => new RegisteringSocket('host-1'),
      openExit: async () => exitSocket,
      dial: async () => site as unknown as Socket,
      traffic,
      capabilities: [SHELL_CAPABILITY, EXIT_CAPABILITY],
      retryMs: 1,
      request: async () => live(),
    })
    links.start()
    await vi.waitFor(() => expect(exitSocket.read).toBe(true))

    exitSocket.deliver(openStream(1, 'example.com:443\nping'))
    await vi.waitFor(() => expect(traffic.connections).toBe(1))
    siteEnd.write('pong!')

    await vi.waitFor(() => expect(traffic.bytes).toBe(4 + 5))
    links.stop()
  })

  /** The daemon closes the exit socket with 1008 when its Session ends,
   *  as it closes the Host socket. Neither link uses that Session again. */
  it('opens no socket with a Session that ended on the exit socket, and opens both after a new sign-in', async () => {
    let inJar = 'the-persons-session'
    const hostSockets: RegisteringSocket[] = []
    const exitSockets: FakeExitSocket[] = []
    const opened: string[] = []
    const exitOpened: string[] = []
    const links = hostLinkFor({
      url: SERVER,
      jar: { get: async () => [{ value: inJar }] },
      credential: () => null,
      open: async (_url, secret) => {
        opened.push(secret)
        const socket = new RegisteringSocket('host-1')
        hostSockets.push(socket)
        return socket
      },
      openExit: async (_url, secret) => {
        exitOpened.push(secret)
        const socket = new FakeExitSocket()
        exitSockets.push(socket)
        return socket
      },
      capabilities: [SHELL_CAPABILITY, EXIT_CAPABILITY],
      retryMs: 1,
      request: async () => live(),
    })

    links.start()
    await vi.waitFor(() => expect(exitOpened).toEqual(['the-persons-session']))
    exitSockets[0].closeWith(1008)
    // The Host socket is still registered with that Session. Many retries
    // of the exit link pass, and none of them uses it.
    await new Promise((resolve) => setTimeout(resolve, 30))
    expect(exitOpened).toEqual(['the-persons-session'])

    // The Host socket drops, and does not open again with that Session.
    hostSockets[0].closeWith(1006)
    await new Promise((resolve) => setTimeout(resolve, 30))
    expect(opened).toEqual(['the-persons-session'])
    expect(exitOpened).toEqual(['the-persons-session'])

    inJar = 'a-new-session'
    await vi.waitFor(() => expect(exitOpened).toEqual(['the-persons-session', 'a-new-session']))
    expect(opened).toEqual(['the-persons-session', 'a-new-session'])
    links.stop()
  })
})

/** The yamux frame of the daemon that opens stream `streamId` with
 *  `bytes`: a data frame with the SYN flag. */
function openStream(streamId: number, bytes: string): Buffer {
  const body = Buffer.from(bytes)
  const head = Buffer.alloc(12)
  head.writeUInt8(0, 0)
  head.writeUInt8(0, 1)
  head.writeUInt16BE(1, 2)
  head.writeUInt32BE(streamId, 4)
  head.writeUInt32BE(body.length, 8)
  return Buffer.concat([head, body])
}

/** The tray turns the Person's Home Exit off (ADR-0029): the client asks
 *  the server to clear the choice of the Person whose Session it holds,
 *  as the Settings card does. */
describe('the Home Exit turned off from the tray', () => {
  it('clears the choice of the Person whose Session the client holds', async () => {
    const request = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) =>
      String(input).endsWith('/api/v1/user')
        ? live()
        : new Response(JSON.stringify({ home_exit: {}, not_switched: [] }), { status: 200 }),
    )

    await turnOffHomeExit({
      url: SERVER,
      jar: { get: async () => [{ value: 'the-persons-session' }] },
      credential: () => null,
      request,
    })

    expect(request).toHaveBeenCalledTimes(2)
    const [target, init] = request.mock.calls[1]
    expect(String(target)).toBe('https://pagis.example.com/api/v1/settings/home-exit')
    expect(init).toMatchObject({
      method: 'DELETE',
      redirect: 'error',
      headers: { cookie: 'pagis_session=the-persons-session' },
    })
  })

  it('says why the server did not turn it off', async () => {
    const request = async (input: RequestInfo | URL) =>
      String(input).endsWith('/api/v1/user')
        ? live()
        : new Response(
            JSON.stringify({ error: { code: 'conflict', message: 'a Local Installation has no Home Exit' } }),
            { status: 409, headers: { 'content-type': 'application/json' } },
          )

    await expect(
      turnOffHomeExit({
        url: SERVER,
        jar: { get: async () => [{ value: 'the-persons-session' }] },
        credential: () => null,
        request,
      }),
    ).rejects.toThrow(/409.*a Local Installation has no Home Exit/)
  })
})
