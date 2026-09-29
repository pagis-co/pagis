import { describe, expect, it } from 'vitest'

import { recoveryView, sameProductOrigin } from './recoveryView'

describe('client recovery view', () => {
  it('returns to the Product App when an owned server recovers', () => {
    expect(recoveryView({ kind: 'running' }, false, true)).toBe('product')
  })

  it('keeps setup until its first handoff and shows later failures', () => {
    expect(recoveryView({ kind: 'running' }, true, false)).toBe('setup')
    expect(recoveryView({ kind: 'running' }, false, false)).toBe('setup')
    expect(recoveryView({ kind: 'failed' }, false, false)).toBe('status')
  })

  it('updates product navigation authority when the recovered port changes', () => {
    expect(sameProductOrigin('http://127.0.0.1:4401', 'http://127.0.0.1:4401/threads')).toBe(true)
    expect(sameProductOrigin('http://127.0.0.1:4401', 'http://127.0.0.1:4400/')).toBe(false)
  })

  /** A connect-only client locks navigation to the server's origin, as
   *  the local one locks it to loopback. */
  it('locks the window to the origin of the server it connected to', () => {
    const server = 'https://pagis.example.com'
    expect(sameProductOrigin(server, 'https://pagis.example.com/channels/01ABC')).toBe(true)
    expect(sameProductOrigin(server, 'http://pagis.example.com/')).toBe(false)
    expect(sameProductOrigin(server, 'https://pagis.example.com.evil.test/')).toBe(false)
    expect(sameProductOrigin(server, 'https://pagis.example.com:8443/')).toBe(false)
    expect(sameProductOrigin(server, 'http://127.0.0.1:4400/')).toBe(false)
  })

  /** A downgrade: the page of a server over TLS sends the window to the
   *  same host over http://, where the network can read and change
   *  what the window loads. */
  it('refuses a navigation from https:// to http:// on the same host', () => {
    const server = new URL('https://pagis.example.com/').origin
    expect(sameProductOrigin(server, 'http://pagis.example.com/')).toBe(false)
    expect(sameProductOrigin(server, 'http://pagis.example.com:443/channels/01ABC')).toBe(false)
    expect(sameProductOrigin(server, 'HTTP://PAGIS.EXAMPLE.COM/')).toBe(false)
    expect(sameProductOrigin(server, 'ws://pagis.example.com/')).toBe(false)
  })
})
