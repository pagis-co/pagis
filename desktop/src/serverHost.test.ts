// The Host wiring of a server this client did not start.
//
// These tests prove that a connected client registers its machine with
// the Session the person's own sign-in left in the jar, against a socket
// a test drives. The last case is a Local Installation: the loopback
// origin passes the origin rule, and the real `openWebSocket` opens the
// Host socket there.

import { EventEmitter } from 'node:events'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { openSignedIn } from './clientSession'
import type { HostSocket } from './host'
import { loopbackOrigin } from './origin'
import { hostLinkFor, hostSession } from './serverHost'
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
describe('a Local Installation with Multi-User Mode off', () => {
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
