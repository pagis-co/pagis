// The byte sockets of a Host: the exit socket and the session socket. Each
// carries one byte stream to the daemon, and each opens only where the
// client trusts the server, as the Host socket does.

import { afterEach, describe, expect, it, vi } from 'vitest'

import { CLOSE, daemonServer } from '../test/websocketDaemon'
import { type ByteSocket, openByteSocket } from './byteSocket'
import { openExitSocket } from './exit'
import { openSessionSocket } from './sessions'

/** A stand-in for the global WebSocket. It records the target of each
 *  socket and opens at once. */
class RecordedWebSocket extends EventTarget {
  static opened: RecordedWebSocket[] = []
  readonly target: string
  binaryType = 'blob'

  constructor(target: URL | string) {
    super()
    this.target = String(target)
    RecordedWebSocket.opened.push(this)
    queueMicrotask(() => this.dispatchEvent(new Event('open')))
  }

  send(): void {}
  close(): void {}
}

type Opener = (url: string, secret: string, hostId: string) => Promise<ByteSocket>

const SOCKETS: Array<[string, Opener, string]> = [
  ['exit', openExitSocket, 'exit'],
  ['session', openSessionSocket, 'sessions'],
]

describe('the byte sockets of a Host', () => {
  const servers: Array<{ close: () => Promise<void> }> = []
  afterEach(async () => {
    RecordedWebSocket.opened = []
    vi.unstubAllGlobals()
    for (const server of servers.splice(0)) await server.close()
  })

  async function daemon() {
    const server = await daemonServer()
    servers.push(server)
    return server
  }

  it.each(SOCKETS)(
    'opens the %s socket over wss:// to a server over TLS, and over ws:// on loopback, on its path of the Host',
    async (_name, open, path) => {
      vi.stubGlobal('WebSocket', RecordedWebSocket)

      await open('https://pagis.example.com/', 'session', 'host-1')
      await open('http://127.0.0.1:4400/', 'session', 'host/../1 2')

      expect(RecordedWebSocket.opened.map((socket) => socket.target)).toEqual([
        `wss://pagis.example.com/api/v1/hosts/host-1/${path}`,
        `ws://127.0.0.1:4400/api/v1/hosts/host%2F..%2F1%202/${path}`,
      ])
      expect(RecordedWebSocket.opened.map((socket) => socket.binaryType)).toEqual(['arraybuffer', 'arraybuffer'])
    },
  )

  it.each(SOCKETS)('opens no ws:// %s socket to another computer', async (_name, open) => {
    vi.stubGlobal('WebSocket', RecordedWebSocket)

    await expect(open('http://192.168.1.10:4400/', 'session', 'host-1')).rejects.toThrow(/https:\/\//)
    await expect(open('http://pagis.example.com/', 'session', 'host-1')).rejects.toThrow(/https:\/\//)

    expect(RecordedWebSocket.opened).toEqual([])
  })

  // As on the Host socket: the daemon refuses an upgrade that a browser
  // page on another origin starts, and the Client App is not a browser
  // page, so it sends neither header.
  it.each(SOCKETS)(
    'sends the Session cookie and no Origin or Sec-Fetch-Site in the handshake of the %s socket',
    async (_name, open, path) => {
      const server = await daemon()

      const socket = await open(server.origin, 'secret', 'host-1')
      socket.close()

      const [end] = server.ends
      expect(end.url).toBe(`/api/v1/hosts/host-1/${path}`)
      expect(end.headers.cookie).toBe('pagis_session=secret')
      expect(end.headers).not.toHaveProperty('origin')
      expect(end.headers).not.toHaveProperty('sec-fetch-site')
    },
  )

  it('hands on the bytes of each binary message, and sends bytes as binary messages', async () => {
    const server = await daemon()
    const socket = await openByteSocket(server.origin, 'secret', '/api/v1/hosts/host-1/exit')
    const received: Buffer[] = []
    socket.onMessage((bytes) => received.push(Buffer.from(bytes)))

    server.ends[0].binary(Buffer.from([0, 1, 2]))
    server.ends[0].binary(Buffer.alloc(70_000, 9))
    socket.send(Buffer.from('to the daemon'))

    await vi.waitFor(() => expect(received).toHaveLength(2))
    expect(received[0]).toEqual(Buffer.from([0, 1, 2]))
    expect(received[1]).toEqual(Buffer.alloc(70_000, 9))
    await vi.waitFor(() => expect(server.ends[0].received().toString()).toBe('to the daemon'))
    socket.close()
  })

  it('closes on a text message, which is not part of the byte stream', async () => {
    const server = await daemon()
    const socket = await openByteSocket(server.origin, 'secret', '/api/v1/hosts/host-1/exit')
    const received: Uint8Array[] = []
    socket.onMessage((bytes) => received.push(bytes))

    server.ends[0].text('hello')

    await vi.waitFor(() => expect(server.ends[0].frames.map((frame) => frame.opcode)).toContain(CLOSE))
    expect(received).toEqual([])
  })

  it('tells the close code of the daemon', async () => {
    const server = await daemon()
    const socket = await openByteSocket(server.origin, 'secret', '/api/v1/hosts/host-1/exit')
    const codes: number[] = []
    socket.onClose((code) => codes.push(code))

    server.ends[0].closeWith(1008)

    await vi.waitFor(() => expect(codes).toEqual([1008]))
  })
})
