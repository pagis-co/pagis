// Where a connected Client App reads its Update (ADR-0027): the feed of
// the release of its server, and no Update past it.

import { describe, expect, it, vi } from 'vitest'

import { serverSource } from './updateSource'

const CLIENT = '1.4.2'
const ORIGIN = 'https://pagis.example.com/'

/** A server whose health route answers `version`. */
function server(version: string) {
  return vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) =>
    new Response(JSON.stringify({ status: 'ok', version }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    }))
}

function sourceOf(fetcher: ReturnType<typeof server>) {
  return serverSource(ORIGIN, CLIENT, fetcher as unknown as typeof fetch)
}

describe('the Update source of a connected Client App', () => {
  /** electron-updater reads the feed of the platform and each file that
   *  it names below the download URL of the release. GitHub answers a
   *  request with several ranges with 501, as the GitHub provider of
   *  electron-updater knows. */
  it('reads the feed of the release of its server when the server is newer', async () => {
    const fetcher = server('1.5.0')

    expect(await sourceOf(fetcher)).toEqual({
      kind: 'feed',
      feed: {
        provider: 'generic',
        url: 'https://github.com/pagis-co/pagis/releases/download/v1.5.0',
        useMultipleRangeRequest: false,
      },
    })
    expect(fetcher).toHaveBeenCalledTimes(1)
    const [url, init] = fetcher.mock.calls[0]
    expect(String(url)).toBe('https://pagis.example.com/api/v1/health')
    expect(init?.redirect).toBe('error')
  })

  /** The Compatibility Range refuses this server, and the message tells
   *  the Person to update Pagis on this computer. The Update to the
   *  release of the server is that update. */
  it('follows a newer server outside its Compatibility Range', async () => {
    expect(await sourceOf(server('2.0.0'))).toMatchObject({
      kind: 'feed',
      feed: { url: 'https://github.com/pagis-co/pagis/releases/download/v2.0.0' },
    })
  })

  it('has no Update when the server runs the same release or an older one', async () => {
    expect(await sourceOf(server(CLIENT))).toEqual({ kind: 'server-not-newer', server: CLIENT })
    expect(await sourceOf(server('1.4.1'))).toEqual({ kind: 'server-not-newer', server: '1.4.1' })
  })

  it('fails with the reason when no Pagis server answers', async () => {
    const fetcher = vi.fn(async () => { throw new Error('connection refused') })

    await expect(serverSource(ORIGIN, CLIENT, fetcher as unknown as typeof fetch)).rejects.toThrow(
      'No Pagis server answered at https://pagis.example.com/, so Pagis cannot read the release of its server.',
    )
  })

  /** The release becomes a part of the URL of the feed. */
  it('refuses a release that is not canonical SemVer', async () => {
    for (const version of ['v1.5.0', '1.5.0+build.1', ' 1.5.0', '1.5.0/../../v9.0.0']) {
      await expect(sourceOf(server(version)), version).rejects.toThrow(
        `The Pagis server at https://pagis.example.com/ reported the release "${version}", which is not a release version.`,
      )
    }
  })
})
