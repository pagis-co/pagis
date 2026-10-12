import { createHash } from 'node:crypto'
import { describe, expect, it } from 'vitest'
import { BrowserSignIns } from './browserSignIn'

const server = { id: 'home', origin: 'https://home.example/', label: 'home.example' }

describe('the browser sign-in of the Client App', () => {
  it('makes a random state and verifier and opens the challenge on the server', () => {
    const signIns = new BrowserSignIns()
    const first = new URL(signIns.begin(server))
    const second = new URL(signIns.begin(server))
    expect(first.origin).toBe('https://home.example')
    expect(first.pathname).toBe('/sign-in')
    expect(first.searchParams.get('app')).toBe('desktop')
    expect(first.searchParams.get('state')).not.toBe(second.searchParams.get('state'))
    const accepted = signIns.accept(`pagis://auth?state=${first.searchParams.get('state')}#bound-secret`)!
    expect(accepted.server).toEqual(server)
    expect(accepted.url).toBe('https://home.example/sign-in#bound-secret')
    expect(accepted.verifier).toMatch(/^[A-Za-z0-9_-]{43}$/)
    expect(first.searchParams.get('challenge')).toBe(createHash('sha256').update(accepted.verifier).digest('base64url'))
  })

  it('refuses unknown, expired and reused state', () => {
    let now = 1000
    const signIns = new BrowserSignIns(() => now)
    expect(signIns.accept('pagis://auth?state=unknown#secret')).toBeNull()
    const expired = new URL(signIns.begin(server)).searchParams.get('state')
    now += 300_000
    expect(signIns.accept(`pagis://auth?state=${expired}#secret`)).toBeNull()
    const good = new URL(signIns.begin(server)).searchParams.get('state')
    const link = `pagis://auth?state=${good}#secret`
    expect(signIns.accept(link)).not.toBeNull()
    expect(signIns.accept(link)).toBeNull()
  })

  it.each(['pagis://other', 'https://auth', 'pagis://auth/path', 'pagis://user@auth', 'pagis://auth:123', 'pagis://auth?state=STATE', 'pagis://auth?state=STATE&state=STATE#secret', 'pagis://auth?state=STATE&origin=https://evil.example#secret', 'not a url', 'pagis://auth?state=STATE#%ZZ'])('opens nothing for %s', (link) => {
    const signIns = new BrowserSignIns()
    const state = new URL(signIns.begin(server)).searchParams.get('state')!
    expect(signIns.accept(link.replaceAll('STATE', state))).toBeNull()
    expect(signIns.accept(`pagis://auth?state=${state}#secret`)).not.toBeNull()
  })

  it('ends pending sign-ins when the server is removed', () => {
    const signIns = new BrowserSignIns()
    const state = new URL(signIns.begin(server)).searchParams.get('state')
    signIns.remove(server.id)
    expect(signIns.accept(`pagis://auth?state=${state}#secret`)).toBeNull()
  })
})
