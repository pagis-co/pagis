import type { HttpOptions, HttpResponse } from '@capacitor/core'
import { describe, expect, it, vi } from 'vitest'

import { connectToServer, type HttpRequest } from './connect'
import { MINIMUM_SERVER_VERSION } from './serverVersion'

const RELEASE = { debug: false }

type Answer = Partial<HttpResponse> & { status: number }

function healthy(signIn: 'password' | 'link' = 'password', version = MINIMUM_SERVER_VERSION): Answer {
  return { status: 200, data: { status: 'ok', version, sign_in: signIn } }
}

/** The setup route answers `410 Gone` once somebody can sign in. */
function setUp(): Answer {
  return { status: 410, data: { error: { code: 'setup_complete' } } }
}

/** A server, by what each route answers, as a fake of `CapacitorHttp.request`. */
function server(answers: Record<string, Answer | (() => Answer)>) {
  return vi.fn(async (options: HttpOptions): Promise<HttpResponse> => {
    const url = new URL(options.url)
    const answer = answers[url.pathname]
    if (!answer) throw new Error(`nothing answers ${url.pathname}`)
    const { status, data = null, headers = {} } = typeof answer === 'function' ? answer() : answer
    return { status, data, headers, url: url.href }
  })
}

function connect(typed: string, request: ReturnType<typeof server> | HttpRequest) {
  return connectToServer(typed, { ...RELEASE, request: request as HttpRequest })
}

function paths(request: ReturnType<typeof server>): string[] {
  return request.mock.calls.map(([options]) => new URL(options.url).pathname)
}

describe('the Connect screen checks a server', () => {
  it('reads the health route, then the setup route, and opens the origin of an address', async () => {
    const request = server({ '/api/v1/health': healthy(), '/api/v1/setup': setUp() })

    const address = await connect('pagis.example.com', request)

    expect(address).toEqual({ origin: 'https://pagis.example.com/', opens: 'https://pagis.example.com/' })
    expect(request.mock.calls.map(([options]) => options.url)).toEqual([
      'https://pagis.example.com/api/v1/health',
      'https://pagis.example.com/api/v1/setup',
    ])
  })

  /** TLS authenticates the server. A redirect could take the app to
   *  another server, or to http://, before it opens the server. */
  it('sends each request as a GET that follows no redirect', async () => {
    const request = server({ '/api/v1/health': healthy(), '/api/v1/setup': setUp() })

    await connect('pagis.example.com', request)

    for (const [options] of request.mock.calls) {
      expect(options.method).toBe('GET')
      expect(options.disableRedirects).toBe(true)
    }
  })

  /** The web view opens the link, and the page of the link trades the
   *  secret. The checks go to the origin, so no request holds the secret. */
  it('checks the server of a Sign-In Link at its origin, and opens the link', async () => {
    const secret = 'not-a-real-link-secret'
    const link = `https://pagis-home.tail1234.ts.net/sign-in#${secret}`
    const request = server({ '/api/v1/health': healthy('link'), '/api/v1/setup': setUp() })

    const address = await connect(link, request)

    expect(address).toEqual({ origin: 'https://pagis-home.tail1234.ts.net/', opens: link })
    for (const [options] of request.mock.calls) expect(JSON.stringify(options)).not.toContain(secret)
  })

  it('sends nothing for an address that the app refuses', async () => {
    const request = server({ '/api/v1/health': healthy(), '/api/v1/setup': setUp() })

    await expect(connect('http://192.168.1.10:4400', request)).rejects.toThrow(/only over https:\/\//)
    await expect(connect('https://pagis.example.com/sign-in', request)).rejects.toThrow(
      /sign-in link is not complete/,
    )

    expect(request).not.toHaveBeenCalled()
  })

  it('says what to do when no Pagis server answers', async () => {
    const message = 'No Pagis server answered at https://pagis.example.com/. Check the address and that the server is running.'
    const refused = vi.fn(async (): Promise<HttpResponse> => {
      throw new Error('connection refused')
    })
    const notFound = server({ '/api/v1/health': { status: 404, data: 'Not Found' } })
    const notPagis = server({ '/api/v1/health': { status: 200, data: '<html>a router</html>' } })

    await expect(connect('pagis.example.com', refused)).rejects.toThrow(message)
    await expect(connect('pagis.example.com', notFound)).rejects.toThrow(message)
    await expect(connect('pagis.example.com', notPagis)).rejects.toThrow(message)
  })

  it('refuses a server below the bound before it asks anything more', async () => {
    const request = server({ '/api/v1/health': healthy('password', '0.0.1'), '/api/v1/setup': setUp() })

    await expect(connect('pagis.example.com', request)).rejects.toThrow(
      `Ask the administrator of the server to update it to ${MINIMUM_SERVER_VERSION} or newer.`,
    )

    expect(paths(request)).toEqual(['/api/v1/health'])
  })

  /** The server's own first run is still open, so nobody can sign in yet. */
  it('points at the administration page when the server has no administrator', async () => {
    const request = server({
      '/api/v1/health': healthy(),
      '/api/v1/setup': { status: 200, data: { administration_origin: 'http://127.0.0.1:4701' } },
    })
    const unnamed = server({ '/api/v1/health': healthy(), '/api/v1/setup': { status: 200, data: {} } })

    await expect(connect('pagis.example.com', request)).rejects.toThrow(
      'This Pagis server has no administrator yet. Finish its setup on the administration page at ' +
        'http://127.0.0.1:4701/ on the server itself, then connect again.',
    )
    await expect(connect('pagis.example.com', unnamed)).rejects.toThrow(
      'This Pagis server has no administrator yet. Finish its setup on the administration page, which ' +
        'answers on the Administration Port of the server itself, then connect again.',
    )
    expect(paths(request)).toEqual(['/api/v1/health', '/api/v1/setup'])
  })

  /** With `disableRedirects`, the native request gives the redirect back
   *  as an answer, and the app goes no further. */
  it.each([['/api/v1/health'], ['/api/v1/setup']])('refuses a redirect of %s', async (route) => {
    const redirect: Answer = { status: 307, headers: { location: `http://pagis.example.com${route}` } }
    const request = server({ '/api/v1/health': healthy(), '/api/v1/setup': setUp(), [route]: redirect })

    await expect(connect('pagis.example.com', request)).rejects.toThrow(
      'The server at https://pagis.example.com/ sent this app to another address. Pagis follows no ' +
        'redirect before it opens a server. Type the address that the server answers on.',
    )

    expect(paths(request).at(-1)).toBe(route)
  })

  /** In Remote Access, the server takes no password from another machine
   *  (ADR-0028). An address alone would open a sign-in page that the
   *  Person cannot use. */
  it('asks for a Sign-In Link when the server signs in this app with a link and the Person typed an address', async () => {
    const request = server({ '/api/v1/health': healthy('link'), '/api/v1/setup': setUp() })

    await expect(connect('pagis-home.tail1234.ts.net', request)).rejects.toThrow(
      'This Pagis server signs in this app with a sign-in link only. Paste a sign-in link of the ' +
        'server. Make a new link in Settings → Sessions on a browser or app that is signed in. Or ask ' +
        'an Administrator for a new invite, or run "pagis pair" on the machine of the server.',
    )

    expect(paths(request)).toEqual(['/api/v1/health', '/api/v1/setup'])
  })

  it('reads a health answer that the native request gives back as text', async () => {
    const request = server({
      '/api/v1/health': {
        status: 200,
        data: JSON.stringify({ status: 'ok', version: MINIMUM_SERVER_VERSION, sign_in: 'password' }),
      },
      '/api/v1/setup': setUp(),
    })

    await expect(connect('pagis.example.com', request)).resolves.toEqual({
      origin: 'https://pagis.example.com/',
      opens: 'https://pagis.example.com/',
    })
  })
})
