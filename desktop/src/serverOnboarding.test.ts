import { describe, expect, it, vi } from 'vitest'

import { assertServerIsReady, connectToServer } from './serverOnboarding'

const CLIENT = '1.4.2'

function healthy(version = CLIENT): Response {
  return new Response(JSON.stringify({ status: 'ok', version }), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  })
}

/** The setup route answers `410 Gone` once somebody can sign in. */
function setUp(): Response {
  return new Response(JSON.stringify({ error: { code: 'setup_complete' } }), { status: 410 })
}

/** A server, by what each route answers. */
function server(answers: Record<string, Response | (() => Response)>) {
  return vi.fn(async (input: RequestInfo | URL, _init?: RequestInit) => {
    const route = new URL(String(input)).pathname
    const answer = answers[route]
    if (!answer) throw new Error(`nothing answers ${route}`)
    return typeof answer === 'function' ? answer() : answer
  })
}

describe('onboarding against a server the client did not start', () => {
  /** The Person signs in on the server's own page in the product
   *  window, so the client sends no sign-in of its own. */
  it('checks the server at the address, and sends no sign-in', async () => {
    const fetcher = server({
      '/api/v1/health': () => healthy(),
      '/api/v1/setup': () => setUp(),
    })

    const address = await connectToServer('pagis.example.com', CLIENT, fetcher as unknown as typeof fetch)

    expect(address).toEqual({ origin: 'https://pagis.example.com/', opens: 'https://pagis.example.com/' })
    const paths = fetcher.mock.calls.map((call) => new URL(String(call[0])).pathname)
    expect(paths).toEqual(['/api/v1/health', '/api/v1/setup'])
    for (const [, init] of fetcher.mock.calls) expect(init?.method ?? 'GET').toBe('GET')
  })

  /** A Sign-In Link of an installation in Remote Access (ADR-0028). The
   *  client checks the server at the origin of the link, and the product
   *  window opens the link itself: the Product App's page at `/sign-in`
   *  trades the secret, so the client sends the secret nowhere. */
  it('checks the server of a Sign-In Link at its origin, and opens the product window at the link', async () => {
    const secret = 'not-a-real-link-secret'
    const link = `https://pagis-home.tail1234.ts.net/sign-in#${secret}`
    const fetcher = server({
      '/api/v1/health': () => healthy(),
      '/api/v1/setup': () => setUp(),
    })

    const address = await connectToServer(link, CLIENT, fetcher as unknown as typeof fetch)

    expect(address).toEqual({ origin: 'https://pagis-home.tail1234.ts.net/', opens: link })
    const sent = fetcher.mock.calls.map((call) => String(call[0]))
    expect(sent).toEqual([
      'https://pagis-home.tail1234.ts.net/api/v1/health',
      'https://pagis-home.tail1234.ts.net/api/v1/setup',
    ])
    for (const [, init] of fetcher.mock.calls) expect(JSON.stringify(init ?? {})).not.toContain(secret)
  })

  /** The link is not spent before the client knows that it can work
   *  with the server: the product window opens it only after the checks. */
  it('refuses a Sign-In Link of a server outside its compatibility range', async () => {
    const fetcher = server({ '/api/v1/health': () => healthy('2.0.0') })

    await expect(connectToServer(
      'https://pagis-home.tail1234.ts.net/sign-in#6f1c0d2e',
      CLIENT,
      fetcher as unknown as typeof fetch,
    )).rejects.toThrow(/Update Pagis on this computer/)
  })

  it('says what to do when no Pagis server answers', async () => {
    const fetcher = vi.fn(async () => { throw new Error('connection refused') })

    await expect(connectToServer('pagis.example.com', CLIENT, fetcher as unknown as typeof fetch)).rejects.toThrow(/No Pagis server answered at https:\/\/pagis.example.com\//)
  })

  /** Quit cancels a check in progress. The check stops at once, and
   *  does not wait for the health probe to time out. */
  it('stops the health probe when the check is cancelled', async () => {
    const fetcher = vi.fn((_input: RequestInfo | URL, init?: RequestInit) => new Promise<Response>((_resolve, reject) => {
      init?.signal?.addEventListener('abort', () => reject(init.signal?.reason))
    }))
    const cancel = new AbortController()

    const checking = connectToServer('pagis.example.com', CLIENT, fetcher as unknown as typeof fetch, cancel.signal)
    cancel.abort()

    await expect(checking).rejects.toThrow(/aborted/)
    expect(fetcher).toHaveBeenCalledTimes(1)
  })

  it('refuses a server outside its compatibility range before it asks anything more', async () => {
    const fetcher = server({
      '/api/v1/health': () => healthy('2.0.0'),
      '/api/v1/setup': () => setUp(),
    })

    await expect(connectToServer('pagis.example.com', CLIENT, fetcher as unknown as typeof fetch)).rejects.toThrow(/Update Pagis on this computer/)

    expect(fetcher.mock.calls).toHaveLength(1)
  })

  /** The server's own first run is still open, so nobody can sign in
   *  yet; the administration port is where it finishes. */
  it('points at the administration page when the server has no administrator', async () => {
    const fetcher = server({
      '/api/v1/health': () => healthy(),
      '/api/v1/setup': () => new Response(JSON.stringify({
        administration_origin: 'http://127.0.0.1:4701',
        providers: ['anthropic'],
        configured_providers: [],
      }), { status: 200 }),
    })

    await expect(connectToServer('pagis.example.com', CLIENT, fetcher as unknown as typeof fetch)).rejects.toThrow(/no administrator yet.*at http:\/\/127\.0\.0\.1:4701\/ on the server itself, then connect again\./s)

    expect(fetcher.mock.calls).toHaveLength(2)
  })

  /** The client refuses the address before it sends a request, so it
   *  opens no product window on a clear-text connection to another
   *  computer. */
  it('sends nothing to an http:// address of another computer', async () => {
    const fetcher = server({
      '/api/v1/health': () => healthy(),
      '/api/v1/setup': () => setUp(),
    })

    await expect(connectToServer('http://192.168.1.10:4400', CLIENT, fetcher as unknown as typeof fetch)).rejects.toThrow(/only over https:\/\//)
    await expect(connectToServer('http://192.168.1.10:4400/sign-in#6f1c0d2e', CLIENT, fetcher as unknown as typeof fetch)).rejects.toThrow(/only over https:\/\//)

    expect(fetcher).not.toHaveBeenCalled()
  })

  it('sends nothing for a Sign-In Link with no secret', async () => {
    const fetcher = server({
      '/api/v1/health': () => healthy(),
      '/api/v1/setup': () => setUp(),
    })

    await expect(connectToServer('https://pagis.example.com/sign-in', CLIENT, fetcher as unknown as typeof fetch)).rejects.toThrow(/sign-in link is not complete/)

    expect(fetcher).not.toHaveBeenCalled()
  })
})

/**
 * An https:// server that answers one route with a redirect to the same
 * path on http://, where a copy of the server answers too.
 *
 * The fetch follows a redirect as `fetch` does, unless the request says
 * `redirect: 'error'`. The fetch records every request it sends.
 */
function redirectingServer(redirected: string) {
  const sent: string[] = []
  const answer = (url: URL): Response => {
    if (url.protocol === 'https:' && url.pathname === redirected) {
      return new Response(null, {
        status: 307,
        headers: { location: `http://${url.host}${url.pathname}` },
      })
    }
    if (url.pathname === '/api/v1/health') return healthy()
    if (url.pathname === '/api/v1/setup') return setUp()
    throw new Error(`nothing answers ${url.href}`)
  }
  const fetcher = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    let url = new URL(String(input))
    for (let hop = 0; hop < 20; hop += 1) {
      sent.push(url.href)
      const response = answer(url)
      const location = response.headers.get('location')
      if (location === null || response.status < 300 || response.status > 399) return response
      if (init?.redirect === 'error') {
        throw new TypeError('fetch failed', { cause: new Error('unexpected redirect') })
      }
      if (init?.redirect === 'manual') return response
      url = new URL(location, url)
    }
    throw new TypeError('fetch failed', { cause: new Error('redirect count exceeded') })
  }
  return { fetcher: fetcher as typeof fetch, sent }
}

/** TLS authenticates the server, and a redirect to http:// would leave
 *  that. The client follows no redirect before the product window
 *  opens. */
describe('an https:// server that redirects to http://', () => {
  it.each([
    ['the health probe', '/api/v1/health'],
    ['the setup check', '/api/v1/setup'],
  ])('fails the connection when it redirects %s, and sends nothing to http://', async (_what, route) => {
    const { fetcher, sent } = redirectingServer(route)

    await expect(connectToServer('https://pagis.example.com', CLIENT, fetcher)).rejects.toThrow()

    expect(sent).toContain(`https://pagis.example.com${route}`)
    expect(sent.filter((url) => !url.startsWith('https://'))).toEqual([])
  })
})

describe('a later start of a connect-only client', () => {
  it('checks the range again, because the administrator owns the version', async () => {
    const ahead = server({ '/api/v1/health': () => healthy('3.0.0') })

    await expect(assertServerIsReady(
      'https://pagis.example.com/', CLIENT, ahead as unknown as typeof fetch,
    )).rejects.toThrow(/Update Pagis on this computer/)

    const fine = server({ '/api/v1/health': () => healthy('1.9.0') })
    await expect(assertServerIsReady(
      'https://pagis.example.com/', CLIENT, fine as unknown as typeof fetch,
    )).resolves.toBeUndefined()
  })
})
