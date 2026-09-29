import { describe, expect, it } from 'vitest'

import { isTrustedServerOrigin, loopbackOrigin, opensInSystemBrowser, serverOrigin } from './origin'

describe('the origin the client talks to', () => {
  it('is the loopback port for a server this client started', () => {
    expect(loopbackOrigin(4400)).toBe('http://127.0.0.1:4400/')
  })

  /** A bare name means TLS, as it does in a browser's address bar: a
   *  server the client did not start is trusted through TLS and the
   *  person's sign-in. */
  it('reads what the person typed as an origin, with TLS by default', () => {
    expect(serverOrigin('pagis.example.com')).toBe('https://pagis.example.com/')
    expect(serverOrigin('  pagis.example.com/team?a=1#x  ')).toBe('https://pagis.example.com/')
    expect(serverOrigin('https://pagis.example.com:8443/')).toBe('https://pagis.example.com:8443/')
  })

  /** Over http:// to another computer, the password, the Session and
   *  every Host command cross the network as clear text. The client
   *  refuses the address before it sends a request. */
  it('refuses http:// to another computer, and names https:// and the setups that give it', () => {
    expect(() => serverOrigin('http://192.168.1.10:4400')).toThrow(/only over https:\/\//)
    expect(() => serverOrigin('http://192.168.1.10:4400'))
      .toThrow(/Caddy, Tailscale Serve or Cloudflare Tunnel/)
    expect(() => serverOrigin('http://pagis.example.com')).toThrow(/only over https:\/\//)
  })

  it('refuses http:// to another computer in any letter case', () => {
    expect(() => serverOrigin('HTTP://192.168.1.10:4400')).toThrow(/only over https:\/\//)
    expect(() => serverOrigin('Http://Pagis.Example.com')).toThrow(/only over https:\/\//)
  })

  /** An SSH tunnel ends on loopback, and the Secure Contexts
   *  specification trusts a loopback origin without TLS. */
  it('keeps http:// on a loopback host', () => {
    expect(serverOrigin('http://127.0.0.1:4400')).toBe('http://127.0.0.1:4400/')
    expect(serverOrigin('http://[::1]:4400')).toBe('http://[::1]:4400/')
    expect(serverOrigin('http://localhost:4400')).toBe('http://localhost:4400/')
    expect(serverOrigin('HTTP://LOCALHOST:4400')).toBe('http://localhost:4400/')
  })

  it('refuses nothing, another scheme and a credential in the address', () => {
    expect(() => serverOrigin('   ')).toThrow(/Enter the address/)
    expect(() => serverOrigin('file:///etc/passwd')).toThrow(/https:\/\/ or http:\/\//)
    expect(() => serverOrigin('ws://pagis.example.com')).toThrow(/https:\/\/ or http:\/\//)
    expect(() => serverOrigin('https://ada:secret@pagis.example.com')).toThrow(/Leave the user name/)
  })
})

describe('the server origin the client trusts', () => {
  it('trusts https:// on any host', () => {
    expect(isTrustedServerOrigin('https://pagis.example.com/')).toBe(true)
    expect(isTrustedServerOrigin('https://192.168.1.10:8443/')).toBe(true)
  })

  it('trusts http:// on a loopback host only', () => {
    expect(isTrustedServerOrigin('http://127.0.0.1:4400/')).toBe(true)
    expect(isTrustedServerOrigin('http://[::1]:4400/')).toBe(true)
    expect(isTrustedServerOrigin('http://localhost:4400/')).toBe(true)
    expect(isTrustedServerOrigin(new URL('http://127.0.0.1:4401/settings'))).toBe(true)

    expect(isTrustedServerOrigin('http://192.168.1.10:4400/')).toBe(false)
    expect(isTrustedServerOrigin('http://pagis.example.com/')).toBe(false)
    expect(isTrustedServerOrigin('HTTP://192.168.1.10:4400/')).toBe(false)
    // A name that only starts like a loopback host is another computer.
    expect(isTrustedServerOrigin('http://localhost.example.com/')).toBe(false)
    expect(isTrustedServerOrigin('http://127.0.0.1.example.com/')).toBe(false)
  })

  it('trusts no other scheme and no text that is not a URL', () => {
    expect(isTrustedServerOrigin('ws://127.0.0.1:4400/')).toBe(false)
    expect(isTrustedServerOrigin('file:///etc/passwd')).toBe(false)
    expect(isTrustedServerOrigin('pagis.example.com')).toBe(false)
    expect(isTrustedServerOrigin('')).toBe(false)
  })
})

/** The start route of a brokered Google authorization, on the Public
 *  Origin that the product window shows. The Person consents in the
 *  system browser and signs in there as themselves. */
const START = '/api/v1/connections/google/start?state=Zx9-abc_123'

describe('the addresses the product window opens in the system browser', () => {
  it('opens https:// and mailto: addresses', () => {
    expect(opensInSystemBrowser('https://docs.example.com/', 'http://127.0.0.1:4400')).toBe(true)
    expect(opensInSystemBrowser(`https://pagis.example.com${START}`, 'https://pagis.example.com')).toBe(true)
    expect(opensInSystemBrowser('mailto:ada@example.com', 'http://127.0.0.1:4400')).toBe(true)
  })

  /** The Public Origin of a Local Installation is loopback over plain
   *  HTTP, under either name of this machine. The start route is there,
   *  on the port of the server that the product window shows. */
  it('opens the start route of a Local Installation under each loopback name', () => {
    for (const product of ['http://127.0.0.1:4400', 'http://127.0.0.1:4400/', 'http://localhost:4400']) {
      expect(opensInSystemBrowser(`http://127.0.0.1:4400${START}`, product)).toBe(true)
      expect(opensInSystemBrowser(`http://localhost:4400${START}`, product)).toBe(true)
    }
    expect(opensInSystemBrowser(`http://[::1]:4400${START}`, 'http://[::1]:4400')).toBe(true)
  })

  /** Nothing authenticates the server of any other http:// address. */
  it('opens no other http:// address', () => {
    // Another program on this machine.
    expect(opensInSystemBrowser(`http://127.0.0.1:8080${START}`, 'http://127.0.0.1:4400')).toBe(false)
    // Another computer.
    expect(opensInSystemBrowser(`http://192.168.1.10:4400${START}`, 'http://127.0.0.1:4400')).toBe(false)
    expect(opensInSystemBrowser('http://pagis.example.com/', 'http://127.0.0.1:4400')).toBe(false)
    // A product window on a server over TLS opens no plain HTTP at all.
    expect(opensInSystemBrowser(`http://127.0.0.1:443${START}`, 'https://pagis.example.com')).toBe(false)
    expect(opensInSystemBrowser(`http://localhost${START}`, 'https://pagis.example.com')).toBe(false)
    // A window with no product origin, such as the Administration Interface.
    expect(opensInSystemBrowser(`http://127.0.0.1:4400${START}`, null)).toBe(false)
  })

  it('opens no other scheme and no text that is not a URL', () => {
    expect(opensInSystemBrowser('file:///etc/passwd', 'http://127.0.0.1:4400')).toBe(false)
    expect(opensInSystemBrowser('javascript:alert(1)', 'http://127.0.0.1:4400')).toBe(false)
    expect(opensInSystemBrowser('ws://127.0.0.1:4400/api/v1/ws', 'http://127.0.0.1:4400')).toBe(false)
    expect(opensInSystemBrowser('not a url', 'http://127.0.0.1:4400')).toBe(false)
  })
})
