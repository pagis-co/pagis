import { describe, expect, it } from 'vitest'

import { isTrustedServerOrigin, serverAddress } from './address'

/** A release build of the Mobile App, and a debug build. */
const RELEASE = { debug: false }
const DEBUG = { debug: true }

/** The secret of a Sign-In Link of the Public Origin (ADR-0028). */
const SECRET = 'not-a-real-link-secret'

describe('what the Person typed in the Server address field', () => {
  /** A bare name means TLS, as it does in the address bar of a browser. */
  it('is an address: the app opens the origin of the server, with TLS by default', () => {
    expect(serverAddress('pagis.example.com', RELEASE)).toEqual({
      origin: 'https://pagis.example.com/',
      opens: 'https://pagis.example.com/',
    })
    expect(serverAddress('  pagis.example.com/team?a=1#x  ', RELEASE)).toEqual({
      origin: 'https://pagis.example.com/',
      opens: 'https://pagis.example.com/',
    })
    expect(serverAddress('https://pagis.example.com:8443/', RELEASE)).toEqual({
      origin: 'https://pagis.example.com:8443/',
      opens: 'https://pagis.example.com:8443/',
    })
  })

  /** The page of the Product App at `/sign-in` posts the secret and puts
   *  the Session cookie in the web view. The app keeps the origin alone. */
  it('is a Sign-In Link: the app keeps the origin, and the web view opens the link', () => {
    const address = serverAddress(`  https://a.example/sign-in#abc  `, RELEASE)

    expect(address).toEqual({ origin: 'https://a.example/', opens: 'https://a.example/sign-in#abc' })
    expect(address.origin).not.toContain('abc')
  })

  it('opens the link on the origin that the app checked, whatever else the Person pasted', () => {
    expect(serverAddress(`HTTPS://Pagis.Example.com:443/sign-in?from=mail#${SECRET}`, RELEASE)).toEqual({
      origin: 'https://pagis.example.com/',
      opens: `https://pagis.example.com/sign-in#${SECRET}`,
    })
  })

  /** A message app or a copy can cut the part after `#` off. The page of
   *  the link would sign nobody in, so the field refuses it. */
  it('refuses a Sign-In Link with no secret', () => {
    for (const typed of [
      'https://pagis.example.com/sign-in',
      'https://pagis.example.com/sign-in#',
      'pagis.example.com/sign-in',
    ]) {
      expect(() => serverAddress(typed, RELEASE), typed).toThrow(
        'This sign-in link is not complete. Copy the whole link, then paste it again.',
      )
    }
  })

  /** Over http:// the secret and the Session would cross the network as
   *  clear text. */
  it('refuses http:// to another computer, and names https:// and the setups that give it', () => {
    for (const typed of [
      'http://a.example',
      'http://192.168.1.10:4400',
      'HTTP://192.168.1.10:4400',
      'Http://Pagis.Example.com',
      `http://pagis.example.com/sign-in#${SECRET}`,
    ]) {
      expect(() => serverAddress(typed, RELEASE), typed).toThrow(/only over https:\/\//)
      expect(() => serverAddress(typed, DEBUG), typed).toThrow(/only over https:\/\//)
    }
    expect(() => serverAddress('http://a.example', RELEASE)).toThrow(/Turn on Remote Access/)
  })

  /** A developer runs a daemon on the computer of the simulator, or
   *  forwards its port to the emulator. A phone in the hands of a Person
   *  has no Pagis server on its own loopback. */
  it('keeps http:// on a loopback host in a debug build only', () => {
    expect(serverAddress('http://127.0.0.1:4400', DEBUG)).toEqual({
      origin: 'http://127.0.0.1:4400/',
      opens: 'http://127.0.0.1:4400/',
    })
    expect(serverAddress('http://[::1]:4400', DEBUG).origin).toBe('http://[::1]:4400/')
    expect(serverAddress('HTTP://LOCALHOST:4400', DEBUG).origin).toBe('http://localhost:4400/')
    expect(serverAddress(`http://127.0.0.1:4400/sign-in#${SECRET}`, DEBUG)).toEqual({
      origin: 'http://127.0.0.1:4400/',
      opens: `http://127.0.0.1:4400/sign-in#${SECRET}`,
    })

    for (const typed of ['http://127.0.0.1:4400', 'http://[::1]:4400', 'http://localhost:4400']) {
      expect(() => serverAddress(typed, RELEASE), typed).toThrow(/only over https:\/\//)
    }
  })

  it('refuses nothing, another scheme and a credential in the address', () => {
    expect(() => serverAddress('   ', RELEASE)).toThrow(/Enter the address/)
    expect(() => serverAddress('file:///etc/passwd', RELEASE)).toThrow(/https:\/\/ or http:\/\//)
    expect(() => serverAddress('ws://pagis.example.com', RELEASE)).toThrow(/https:\/\/ or http:\/\//)
    expect(() => serverAddress('ftp://pagis.example.com/sign-in#x', RELEASE)).toThrow(/https:\/\/ or http:\/\//)
    expect(() => serverAddress('https://ada:secret@pagis.example.com', RELEASE)).toThrow(/Leave the user name/)
    expect(() => serverAddress(`https://ada:pw@pagis.example.com/sign-in#${SECRET}`, RELEASE)).toThrow(/Leave the user name/)
  })
})

describe('the server origin the app trusts', () => {
  it('trusts https:// on any host', () => {
    expect(isTrustedServerOrigin('https://pagis.example.com/', RELEASE)).toBe(true)
    expect(isTrustedServerOrigin('https://192.168.1.10:8443/', RELEASE)).toBe(true)
  })

  it('trusts http:// on a loopback host in a debug build only', () => {
    for (const origin of ['http://127.0.0.1:4400/', 'http://[::1]:4400/', 'http://localhost:4400/']) {
      expect(isTrustedServerOrigin(origin, DEBUG), origin).toBe(true)
      expect(isTrustedServerOrigin(origin, RELEASE), origin).toBe(false)
    }

    expect(isTrustedServerOrigin('http://192.168.1.10:4400/', DEBUG)).toBe(false)
    expect(isTrustedServerOrigin('http://pagis.example.com/', DEBUG)).toBe(false)
    // A name that only starts like a loopback host is another computer.
    expect(isTrustedServerOrigin('http://localhost.example.com/', DEBUG)).toBe(false)
    expect(isTrustedServerOrigin('http://127.0.0.1.example.com/', DEBUG)).toBe(false)
  })

  it('trusts no other scheme and no text that is not a URL', () => {
    expect(isTrustedServerOrigin('ws://127.0.0.1:4400/', DEBUG)).toBe(false)
    expect(isTrustedServerOrigin('file:///etc/passwd', DEBUG)).toBe(false)
    expect(isTrustedServerOrigin('pagis.example.com', DEBUG)).toBe(false)
    expect(isTrustedServerOrigin('', DEBUG)).toBe(false)
  })
})
