// The sign-in of a server this client did not start. The Person signs in
// on the server's own page in the product window, and the server's answer
// puts the Session cookie in the jar of the client's Electron session.
// These tests drive a jar that reports each change, as Electron's does.

import { EventEmitter } from 'node:events'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { type ChangedCookie, openServerPage, watchServerSignIn } from './serverSignIn'

const SERVER = 'https://pagis.example.com/'

interface StoredCookie extends ChangedCookie {
  host: string
}

/** A cookie jar that keeps one cookie for each host and name, and reports
 *  each change, as the cookie store of an Electron session does. */
class Jar extends EventEmitter {
  readonly cookies = new Map<string, StoredCookie>()
  readonly sets: Record<string, unknown>[] = []

  async get(filter: { url: string; name: string }): Promise<StoredCookie[]> {
    const cookie = this.cookies.get(`${new URL(filter.url).host}/${filter.name}`)
    return cookie === undefined ? [] : [cookie]
  }

  async set(cookie: { url: string; name: string; value: string; secure?: boolean; expirationDate?: number }): Promise<void> {
    this.sets.push(cookie)
    this.store(cookie.url, { name: cookie.name, value: cookie.value, secure: cookie.secure === true, expirationDate: cookie.expirationDate })
  }

  /** What the answer of a server puts in the jar: a sign-in on its page. */
  answer(url: string, cookie: ChangedCookie): void {
    this.store(url, cookie)
  }

  /** A sign-out: the server ends the cookie. */
  end(url: string, name: string): void {
    const key = `${new URL(url).host}/${name}`
    const cookie = this.cookies.get(key)
    if (cookie === undefined) return
    this.cookies.delete(key)
    this.emit('changed', {}, cookie, 'expired', true)
  }

  private store(url: string, cookie: ChangedCookie): void {
    const host = new URL(url).host
    const key = `${host}/${cookie.name}`
    const before = this.cookies.get(key)
    if (before !== undefined) this.emit('changed', {}, before, 'overwrite', true)
    const stored = { ...cookie, host }
    this.cookies.set(key, stored)
    this.emit('changed', {}, stored, 'explicit', false)
  }
}

const stops: (() => void)[] = []
afterEach(() => { while (stops.length > 0) stops.pop()!() })

function watch(url: string, jar: Jar, signedIn: () => void): () => void {
  const stop = watchServerSignIn(url, jar, signedIn)
  stops.push(stop)
  return stop
}

describe('the sign-in on the page of a server', () => {
  it('says nothing until the Person signs in, then says it once', async () => {
    const jar = new Jar()
    const signedIn = vi.fn()

    watch(SERVER, jar, signedIn)
    await vi.waitFor(() => expect(jar.listenerCount('changed')).toBe(1))
    expect(signedIn).not.toHaveBeenCalled()

    jar.answer(SERVER, { name: 'pagis_session', value: 'adas-session', secure: true })
    await vi.waitFor(() => expect(signedIn).toHaveBeenCalledTimes(1))
    jar.answer(SERVER, { name: 'pagis_session', value: 'adas-next-session', secure: true })
    await new Promise((resolve) => setTimeout(resolve, 10))

    expect(signedIn).toHaveBeenCalledTimes(1)
  })

  /** A later start: the Session of the last sign-in is in the jar. */
  it('says it at once when the jar already holds a Session of the server', async () => {
    const jar = new Jar()
    jar.answer(SERVER, { name: 'pagis_session', value: 'adas-session', secure: true })
    const signedIn = vi.fn()

    watch(SERVER, jar, signedIn)

    await vi.waitFor(() => expect(signedIn).toHaveBeenCalledTimes(1))
  })

  it('ignores a cookie of another name or of another server, and a sign-out', async () => {
    const jar = new Jar()
    const signedIn = vi.fn()
    watch(SERVER, jar, signedIn)

    jar.answer(SERVER, { name: 'theme', value: 'dark', secure: true })
    jar.answer('https://other.example.com/', { name: 'pagis_session', value: 'elsewhere', secure: true })
    jar.answer(SERVER, { name: 'pagis_session', value: 'adas-session', secure: true })
    await vi.waitFor(() => expect(signedIn).toHaveBeenCalledTimes(1))
    const again = vi.fn()
    watch(SERVER, jar, again)
    await vi.waitFor(() => expect(again).toHaveBeenCalledTimes(1))

    jar.end(SERVER, 'pagis_session')
    const afterSignOut = vi.fn()
    watch(SERVER, jar, afterSignOut)
    await new Promise((resolve) => setTimeout(resolve, 10))

    expect(afterSignOut).not.toHaveBeenCalled()
  })

  it('says nothing after it stops', async () => {
    const jar = new Jar()
    const signedIn = vi.fn()

    watch(SERVER, jar, signedIn)()
    jar.answer(SERVER, { name: 'pagis_session', value: 'adas-session', secure: true })
    await new Promise((resolve) => setTimeout(resolve, 10))

    expect(signedIn).not.toHaveBeenCalled()
    expect(jar.listenerCount('changed')).toBe(0)
  })
})

/** A proxy that does not report TLS makes the server set no `Secure`
 *  (ADR-0024). The jar still sends the cookie over TLS only. */
describe('the Session cookie of an https:// server', () => {
  it('holds a cookie that the server set with no Secure as Secure, with its value and its end', async () => {
    const jar = new Jar()
    const signedIn = vi.fn()
    watch(SERVER, jar, signedIn)

    jar.answer(SERVER, { name: 'pagis_session', value: 'adas-session', secure: false, expirationDate: 1_900_000_000 })

    await vi.waitFor(() => expect(jar.sets).toEqual([{
      url: SERVER,
      name: 'pagis_session',
      value: 'adas-session',
      httpOnly: true,
      sameSite: 'strict',
      secure: true,
      expirationDate: 1_900_000_000,
    }]))
    await vi.waitFor(() => expect(signedIn).toHaveBeenCalledTimes(1))
    expect((await jar.get({ url: SERVER, name: 'pagis_session' }))[0]?.secure).toBe(true)
  })

  it('also holds a later sign-in as Secure', async () => {
    const jar = new Jar()
    watch(SERVER, jar, () => {})
    jar.answer(SERVER, { name: 'pagis_session', value: 'first', secure: true })
    jar.end(SERVER, 'pagis_session')

    jar.answer(SERVER, { name: 'pagis_session', value: 'second', secure: false })

    await vi.waitFor(() => expect(jar.sets).toEqual([expect.objectContaining({ value: 'second', secure: true })]))
  })

  /** A browser sends no `Secure` cookie over http://, and a loopback
   *  server, such as an SSH tunnel, is http://. */
  it('leaves the cookie of a loopback http:// server as it is', async () => {
    const tunnel = 'http://127.0.0.1:4400/'
    const jar = new Jar()
    const signedIn = vi.fn()
    watch(tunnel, jar, signedIn)

    jar.answer(tunnel, { name: 'pagis_session', value: 'adas-session', secure: false })

    await vi.waitFor(() => expect(signedIn).toHaveBeenCalledTimes(1))
    expect(jar.sets).toEqual([])
  })
})

/** The error of an Electron load whose page failed, as `loadURL` rejects
 *  with it. */
function loadFailure(code: string, errno: number, url: string): Error {
  return Object.assign(new Error(`${code} (${errno}) loading '${url}'`), { code, errno, url })
}

describe('the product window on a server this client did not start', () => {
  const LINK = `${SERVER}sign-in#6f1c0d2e`

  /** The page of a Sign-In Link opens the app once the server signs it
   *  in, which can be before its own load finished. Electron then
   *  rejects the load of the link with ERR_ABORTED, and the window
   *  shows the app. */
  it('opens the page of a Sign-In Link that goes on to the app before its load finished', async () => {
    const window = { loadURL: vi.fn(async () => { throw loadFailure('ERR_ABORTED', -3, SERVER) }) }

    await expect(openServerPage(window, LINK)).resolves.toBeUndefined()

    expect(window.loadURL).toHaveBeenCalledWith(LINK)
  })

  it('fails when the page does not load', async () => {
    const window = { loadURL: vi.fn(async () => { throw loadFailure('ERR_CONNECTION_REFUSED', -102, SERVER) }) }

    await expect(openServerPage(window, SERVER)).rejects.toThrow(/ERR_CONNECTION_REFUSED/)
  })
})
