// The count of the Runs that "Restart to Update" stops (ADR-0027). The
// Administration Port answers it to an Administrator.

import { describe, expect, it, vi } from 'vitest'

import { unfinishedRuns } from './unfinishedRuns'

const PRODUCT = 'http://127.0.0.1:4400/'
const ADMINISTRATION = 'http://127.0.0.1:4401/'
const CREDENTIAL = 'a'.repeat(64)

/** The server: the Session `live` is good, and the health route answers
 *  `health` to a request that carries a good Session. */
function server(health: () => Response, live = 'held') {
  return vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input)
    const cookie = new Headers(init?.headers).get('cookie')
    if (url === `${PRODUCT}api/v1/user`) {
      return new Response('{}', { status: cookie === `pagis_session=${live}` ? 200 : 401 })
    }
    if (url === `${PRODUCT}api/v1/sessions/client`) {
      return new Response('{}', { status: 200, headers: { 'set-cookie': `pagis_session=${live}; Max-Age=60` } })
    }
    if (url === `${ADMINISTRATION}api/v1/administration/health`) {
      return cookie === `pagis_session=${live}` ? health() : new Response(null, { status: 401 })
    }
    return new Response(null, { status: 404 })
  })
}

function health(body: unknown): () => Response {
  return () => new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } })
}

function jar(held: string | null) {
  return {
    get: vi.fn(async () => (held === null ? [] : [{ value: held }])),
    set: vi.fn(async () => undefined),
  }
}

describe('the Runs that a restart stops', () => {
  it('reads the count with the Session that the client holds', async () => {
    const request = server(health({ unfinished_runs: 3, queued_runs: 1 }))
    const cookies = jar('held')

    expect(await unfinishedRuns(ADMINISTRATION, PRODUCT, CREDENTIAL, cookies, request)).toBe(3)
    expect(request.mock.calls.map(([input]) => String(input))).not.toContain(`${PRODUCT}api/v1/sessions/client`)
  })

  it('trades the Client Credential when the client holds no live Session, and keeps the new one', async () => {
    const request = server(health({ unfinished_runs: 0 }), 'fresh')
    const cookies = jar('spent')

    expect(await unfinishedRuns(ADMINISTRATION, PRODUCT, CREDENTIAL, cookies, request)).toBe(0)
    expect(cookies.set).toHaveBeenCalledWith(expect.objectContaining({ url: PRODUCT, name: 'pagis_session', value: 'fresh' }))
  })

  it('has no count when the route does not answer or answers no count', async () => {
    const offline = vi.fn(async () => { throw new TypeError('fetch failed') })
    expect(await unfinishedRuns(ADMINISTRATION, PRODUCT, CREDENTIAL, jar('held'), offline)).toBeNull()

    const refused = server(() => new Response(null, { status: 503 }))
    expect(await unfinishedRuns(ADMINISTRATION, PRODUCT, CREDENTIAL, jar('held'), refused)).toBeNull()

    for (const body of [{}, { unfinished_runs: -1 }, { unfinished_runs: 1.5 }, { unfinished_runs: '2' }]) {
      const request = server(health(body))
      expect(await unfinishedRuns(ADMINISTRATION, PRODUCT, CREDENTIAL, jar('held'), request)).toBeNull()
    }
  })

  it('has no count with no Session and no Client Credential', async () => {
    const request = server(health({ unfinished_runs: 2 }))

    expect(await unfinishedRuns(ADMINISTRATION, PRODUCT, null, jar(null), request)).toBeNull()
  })
})
