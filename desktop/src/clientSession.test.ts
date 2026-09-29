import * as os from 'node:os'
import { describe, expect, it, vi } from 'vitest'

import {
  exchangeClientCredential,
  openSignedIn,
  openSignedInAt,
  openWithSession,
  reusableSession,
  sessionInJar,
} from './clientSession'

const URL_UNDER_TEST = 'http://127.0.0.1:4400/'

/** A cookie jar with no Session in it, which is every first launch. */
const emptyJar = async () => []

function signedIn(secret = 'session-secret'): Response {
  return new Response(JSON.stringify({ id: 'user-1' }), {
    status: 200,
    headers: {
      'content-type': 'application/json',
      'set-cookie': `pagis_session=${secret}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000`,
    },
  })
}

describe('the Client Credential exchange', () => {
  it('posts the credential and reads the session out of the cookie', async () => {
    const request = vi.fn(async () => signedIn())

    const secret = await exchangeClientCredential(URL_UNDER_TEST, 'a'.repeat(64), undefined, request)

    expect(secret).toEqual({ secret: 'session-secret', expiresAt: expect.any(Number) })
    const [input, init] = request.mock.calls[0] as unknown as [RequestInfo | URL, RequestInit]
    expect(String(input)).toBe('http://127.0.0.1:4400/api/v1/sessions/client')
    expect(init.method).toBe('POST')
    expect(init.body).toBe(JSON.stringify({ credential: 'a'.repeat(64), client_name: os.hostname() }))
  })

  it('refuses a wrong credential and an answer without a cookie', async () => {
    await expect(exchangeClientCredential(
      URL_UNDER_TEST, 'wrong', undefined, async () => new Response(null, { status: 401 }),
    )).rejects.toThrow(/HTTP 401/)
    await expect(exchangeClientCredential(
      URL_UNDER_TEST, 'a'.repeat(64), undefined, async () => new Response('{}', { status: 200 }),
    )).rejects.toThrow(/no session/)
  })
})

describe('the handoff into the product window', () => {
  it('puts the session in the jar before the window loads the daemon URL', async () => {
    const order: string[] = []
    const request = vi.fn(async () => { order.push('exchange'); return signedIn('fresh') })
    const jar = { set: vi.fn(async () => { order.push('cookie') }), get: emptyJar }
    const window = { loadURL: vi.fn(async () => { order.push('load') }) }

    await openSignedIn(URL_UNDER_TEST, 'a'.repeat(64), jar, window, request)

    expect(order).toEqual(['exchange', 'cookie', 'load'])
    expect(jar.set).toHaveBeenCalledWith({
      url: URL_UNDER_TEST,
      name: 'pagis_session',
      value: 'fresh',
      httpOnly: true,
      sameSite: 'strict',
      expirationDate: expect.any(Number),
    })
    expect(window.loadURL).toHaveBeenCalledWith(URL_UNDER_TEST)
  })

  /** The Administration Interface is a second port of the same host and
   *  holds no credential exchange of its own: the Session is
   *  traded on the product port and set for the administration one. */
  it('trades the session on the product port and sets it for the administration port', async () => {
    const administration = 'http://127.0.0.1:4401/'
    const request = vi.fn(async () => signedIn('fresh'))
    const jar = { set: vi.fn(async () => {}), get: emptyJar }
    const window = { loadURL: vi.fn(async () => {}) }

    await openSignedInAt(administration, URL_UNDER_TEST, 'a'.repeat(64), jar, window, request)

    const [input] = request.mock.calls[0] as unknown as [RequestInfo | URL]
    expect(String(input)).toBe('http://127.0.0.1:4400/api/v1/sessions/client')
    expect(jar.set).toHaveBeenCalledWith({
      url: administration,
      name: 'pagis_session',
      value: 'fresh',
      httpOnly: true,
      sameSite: 'strict',
      expirationDate: expect.any(Number),
    })
    expect(window.loadURL).toHaveBeenCalledWith(administration)
  })

  it('does not load the window when the exchange fails', async () => {
    const jar = { set: vi.fn(async () => {}), get: emptyJar }
    const window = { loadURL: vi.fn(async () => {}) }

    await expect(openSignedIn(
      URL_UNDER_TEST, 'wrong', jar, window, async () => new Response(null, { status: 401 }),
    )).rejects.toThrow(/HTTP 401/)

    expect(jar.set).not.toHaveBeenCalled()
    expect(window.loadURL).not.toHaveBeenCalled()
  })
})

/** The server sets `Secure` on the Session cookie over TLS, and the jar
 *  holds the cookie as the server set it. */
describe('the Session cookie over TLS', () => {
  function signedInOverTls(attributes: string): Response {
    return new Response(JSON.stringify({ id: 'user-1' }), {
      status: 200,
      headers: {
        'content-type': 'application/json',
        'set-cookie': `pagis_session=tls-session; ${attributes}`,
      },
    })
  }

  it('keeps the Secure attribute in the jar', async () => {
    const jar = { set: vi.fn(async () => {}) }
    const window = { loadURL: vi.fn(async () => {}) }

    const held = await exchangeClientCredential(
      'https://pagis.example.com/', 'a'.repeat(64), undefined,
      async () => signedInOverTls('Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=2592000'),
    )
    await openWithSession('https://pagis.example.com/', held, jar, window)

    expect(jar.set).toHaveBeenCalledWith({
      url: 'https://pagis.example.com/',
      name: 'pagis_session',
      value: 'tls-session',
      httpOnly: true,
      sameSite: 'strict',
      secure: true,
      expirationDate: expect.any(Number),
    })
  })

  it('reads the attribute in any letter case and at the end of the header', async () => {
    const held = await exchangeClientCredential(
      'https://pagis.example.com/', 'a'.repeat(64), undefined,
      async () => signedInOverTls('Path=/; HttpOnly; SameSite=Strict; SECURE'),
    )

    expect(held).toEqual({ secret: 'tls-session', secure: true })
  })

  /** A proxy that does not report TLS makes the server set no `Secure`.
   *  The origin is still https://, so the jar sends the cookie over TLS
   *  only. A value that only contains the word is not the attribute. */
  it('makes the cookie Secure on an https:// origin that set no Secure', async () => {
    const jar = { set: vi.fn(async () => {}) }
    const window = { loadURL: vi.fn(async () => {}) }

    const held = await exchangeClientCredential(
      'https://pagis.example.com/', 'a'.repeat(64), undefined,
      async () => signedInOverTls('Path=/secure; HttpOnly; SameSite=Strict'),
    )
    await openWithSession('https://pagis.example.com/', held, jar, window)

    expect(held).toEqual({ secret: 'tls-session' })
    expect(jar.set).toHaveBeenCalledWith(expect.objectContaining({ secure: true }))
  })

  /** A browser sends no `Secure` cookie over http://, and the loopback
   *  origin of a local installation is http://. */
  it('makes no cookie Secure on a loopback http:// origin', async () => {
    const jar = { set: vi.fn(async () => {}) }
    const window = { loadURL: vi.fn(async () => {}) }

    await openWithSession('http://127.0.0.1:4400/', { secret: 'local-session' }, jar, window)

    const [cookie] = jar.set.mock.calls[0] as unknown as [Record<string, unknown>]
    expect(cookie).not.toHaveProperty('secure')
  })
})

describe('the Session in the jar', () => {
  it('reads the session the product window signed in with, for the host socket', async () => {
    const jar = { get: vi.fn(async () => [{ value: 'server-session' }]) }

    expect(await sessionInJar('https://pagis.example.com/', jar)).toEqual({ secret: 'server-session' })
    expect(jar.get).toHaveBeenCalledWith({
      url: 'https://pagis.example.com/',
      name: 'pagis_session',
    })
    expect(await sessionInJar('https://pagis.example.com/', { get: async () => [] })).toBeNull()
  })

  /** The server's own sign-in page sets the cookie in the jar, so the
   *  client reads from the jar whether the cookie is `Secure`. */
  it('says whether the cookie in the jar is Secure', async () => {
    const jar = { get: async () => [{ value: 'server-session', secure: true, expirationDate: 1_900_000_000 }] }

    expect(await sessionInJar('https://pagis.example.com/', jar))
      .toEqual({ secret: 'server-session', expiresAt: 1_900_000_000, secure: true })
  })
})

describe('one launch opens one Session', () => {
  /** A launch trades the Client Credential at most once, not once for
   *  the product window, the Host socket and the administration window.
   *  Each Session row lives thirty days. */
  it('reuses the Session in the jar and trades nothing', async () => {
    const request = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) =>
      new Response(JSON.stringify({ id: 'user-1' }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }))
    const jar = { get: vi.fn(async () => [{ value: 'the-live-session' }]) }

    const secret = await reusableSession(URL_UNDER_TEST, jar, request)

    expect(secret).toEqual({ secret: 'the-live-session' })
    const [input, init] = request.mock.calls[0] as unknown as [RequestInfo | URL, RequestInit]
    expect(String(input)).toBe('http://127.0.0.1:4400/api/v1/user')
    expect(init.headers).toEqual({ cookie: 'pagis_session=the-live-session' })
  })

  it('reuses nothing when the jar is empty, the cookie is spent, or the daemon does not answer', async () => {
    expect(await reusableSession(URL_UNDER_TEST, { get: emptyJar }, async () => signedIn()))
      .toBeNull()

    const held = { get: async () => [{ value: 'the-spent-session' }] }
    expect(await reusableSession(
      URL_UNDER_TEST, held, async () => new Response(null, { status: 401 }),
    )).toBeNull()
    expect(await reusableSession(URL_UNDER_TEST, held, async () => {
      throw new Error('the daemon is not there')
    })).toBeNull()
  })

  /** The product window and the administration window are two ports of
   *  one host, so one Session serves both. */
  it('opens the administration window on the Session the product window holds', async () => {
    const administration = 'http://127.0.0.1:4401/'
    const request = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) =>
      new Response(JSON.stringify({ id: 'user-1' }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }))
    const jar = { set: vi.fn(async () => {}), get: async () => [{ value: 'the-live-session' }] }
    const window = { loadURL: vi.fn(async () => {}) }

    await openSignedInAt(administration, URL_UNDER_TEST, 'a'.repeat(64), jar, window, request)

    const asked = request.mock.calls.map((call) => String(call[0]))
    expect(asked.some((url) => url.endsWith('/api/v1/sessions/client'))).toBe(false)
    expect(jar.set).toHaveBeenCalledWith({
      url: administration,
      name: 'pagis_session',
      value: 'the-live-session',
      httpOnly: true,
      sameSite: 'strict',
    })
    expect(window.loadURL).toHaveBeenCalledWith(administration)
  })
})
