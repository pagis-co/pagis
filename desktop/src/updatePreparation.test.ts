// The preparation of a restart to an Update (ADR-0027): the Client App of
// a Local Installation reads the Runtime Lock of the next release, puts its
// Server Package into the download cache, and asks the daemon to pull its
// Computer Image. A failed step does not stop the Update.

import { describe, expect, it, vi } from 'vitest'

import type { RuntimeLock } from './runtimeLock'
import { prepareUpdate, pullComputerImage, releaseLock } from './updatePreparation'

const SHA = 'b'.repeat(64)
const IMAGE = `ghcr.io/pagis-co/pagis-computer@sha256:${'c'.repeat(64)}`
const LINUX = { platform: 'linux', arch: 'x64' }

/** The Linux x64 Runtime Lock that the release `release` publishes. */
function linuxLock(release: string): Record<string, unknown> {
  const name = `pagis-server-${release}-x86_64-unknown-linux-gnu.tar.gz`
  return {
    schema: 1,
    release,
    platform: 'linux',
    arch: 'x64',
    asset: {
      format: 'tar.gz',
      name,
      url: `https://github.com/pagis-co/pagis/releases/download/v${release}/${name}`,
      size: 123,
      sha256: SHA,
    },
    entries: [
      { path: 'pagis', kind: 'executable', size: 10, sha256: SHA, mode: 0o755 },
      { path: 'gog', kind: 'executable', size: 11, sha256: SHA, mode: 0o755 },
      { path: 'LICENSE', kind: 'file', size: 12, sha256: SHA, mode: 0o644 },
      { path: 'LICENSE.gog', kind: 'file', size: 12, sha256: SHA, mode: 0o644 },
      { path: 'THIRD_PARTY_NOTICES', kind: 'file', size: 13, sha256: SHA, mode: 0o644 },
    ],
    computer_image: IMAGE,
  }
}

/** The steps of a preparation, each a fake that records its call. */
function steps(lock: Promise<RuntimeLock>) {
  return {
    readLock: vi.fn(() => lock),
    download: vi.fn(async (_lock: RuntimeLock) => {}),
    pull: vi.fn(async (_image: string) => {}),
  }
}

describe('the preparation of a restart to an Update', () => {
  it('reads the lock of the Update, downloads its Server Package and asks for its Computer Image', async () => {
    const lock = linuxLock('1.1.0') as unknown as RuntimeLock
    const prepared = steps(Promise.resolve(lock))
    const log = vi.fn()

    await prepareUpdate('1.1.0', prepared, log)

    expect(prepared.readLock).toHaveBeenCalledWith('1.1.0')
    expect(prepared.download).toHaveBeenCalledWith(lock)
    expect(prepared.pull).toHaveBeenCalledWith(IMAGE)
    expect(log).not.toHaveBeenCalled()
  })

  it('logs a lock that it cannot read, and does nothing more', async () => {
    const prepared = steps(Promise.reject(new Error('HTTP 404')))
    const log = vi.fn()

    await prepareUpdate('1.1.0', prepared, log)

    expect(prepared.download).not.toHaveBeenCalled()
    expect(prepared.pull).not.toHaveBeenCalled()
    expect(log).toHaveBeenCalledWith(expect.stringMatching(/Runtime Lock of Pagis 1\.1\.0.*HTTP 404/))
  })

  it('asks for the pull when the download fails, and logs each failure', async () => {
    const prepared = steps(Promise.resolve(linuxLock('1.1.0') as unknown as RuntimeLock))
    prepared.download.mockRejectedValue(new Error('server download failed with HTTP 404'))
    prepared.pull.mockRejectedValue(new Error('Docker does not answer'))
    const log = vi.fn()

    await prepareUpdate('1.1.0', prepared, log)

    expect(prepared.pull).toHaveBeenCalledWith(IMAGE)
    expect(log).toHaveBeenCalledWith(expect.stringMatching(/Server Package of Pagis 1\.1\.0.*HTTP 404/))
    expect(log).toHaveBeenCalledWith(expect.stringMatching(/Computer Image of Pagis 1\.1\.0.*Docker does not answer/))
  })
})

describe('the Runtime Lock of an Update', () => {
  it('reads the lock of this platform from the release of the Update', async () => {
    const request = vi.fn(async (_input: RequestInfo | URL) => new Response(JSON.stringify(linuxLock('1.1.0'))))

    const lock = await releaseLock('1.1.0', request, LINUX)

    expect(lock).toMatchObject({ release: '1.1.0', computer_image: IMAGE })
    expect(String(request.mock.calls[0][0])).toBe(
      'https://github.com/pagis-co/pagis/releases/download/v1.1.0/runtime-lock-linux-x64.json',
    )
  })

  it('refuses a lock of another release and an answer that is not a lock', async () => {
    const other = vi.fn(async () => new Response(JSON.stringify(linuxLock('1.0.9'))))
    await expect(releaseLock('1.1.0', other, LINUX)).rejects.toThrow(/release/)

    const missing = vi.fn(async () => new Response('Not Found', { status: 404 }))
    await expect(releaseLock('1.1.0', missing, LINUX)).rejects.toThrow(/HTTP 404/)
  })

  it('reads nothing for a version that is not a release version', async () => {
    const request = vi.fn(async () => new Response('{}'))

    for (const version of ['../../other', 'v1.1.0', '']) {
      await expect(releaseLock(version, request, LINUX)).rejects.toThrow(/not a release version/)
    }
    expect(request).not.toHaveBeenCalled()
  })
})

describe('the pull of the Computer Image of an Update', () => {
  const PRODUCT = 'http://127.0.0.1:4400/'
  const ADMINISTRATION = 'http://127.0.0.1:4401/'
  const PULL = `${ADMINISTRATION}api/v1/administration/computer-image/pull`

  /** The daemon: the Session `held` is good, and the pull route answers
   *  `answer` to a request that carries it. */
  function daemon(answer: () => Response) {
    return vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input)
      const cookie = new Headers(init?.headers).get('cookie')
      if (url === `${PRODUCT}api/v1/user`) {
        return new Response('{}', { status: cookie === 'pagis_session=held' ? 200 : 401 })
      }
      if (url === PULL) return cookie === 'pagis_session=held' ? answer() : new Response(null, { status: 401 })
      return new Response(null, { status: 404 })
    })
  }

  const installation = {
    administrationUrl: ADMINISTRATION,
    productUrl: PRODUCT,
    credential: 'a'.repeat(64),
    jar: { get: async () => [{ value: 'held' }], set: async () => undefined },
  }

  it('asks the daemon on the Administration Port, and waits for the answer', async () => {
    const request = daemon(() => new Response(JSON.stringify({ image: IMAGE }), { status: 200 }))

    await pullComputerImage(IMAGE, installation, request)

    const pull = request.mock.calls.find(([input]) => String(input) === PULL)
    expect(pull?.[1]?.method).toBe('POST')
    expect(JSON.parse(String(pull?.[1]?.body))).toEqual({ image: IMAGE })
  })

  it('fails with the reason that the daemon gives', async () => {
    const request = daemon(() => new Response(
      JSON.stringify({ error: { code: 'docker_unavailable', message: 'Docker does not answer: no endpoint answered' } }),
      { status: 503 },
    ))

    await expect(pullComputerImage(IMAGE, installation, request)).rejects.toThrow(
      /HTTP 503: Docker does not answer: no endpoint answered/,
    )
  })
})
