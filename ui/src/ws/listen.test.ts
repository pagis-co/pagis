// The listen-live socket contract (ADR-0020): a cookie handshake,
// `ready` with the codec, one binary message per 20 ms G.711 frame
// decoded to samples, and `ended` when the call ends.

import { beforeEach, describe, expect, it } from 'vitest'

import { Listen, decodeAlaw, decodeG711, decodeUlaw } from './listen'

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  sent: string[] = []
  closed = false
  readyState = 0
  binaryType = 'blob'
  onopen: (() => void) | null = null
  onmessage: ((event: { data: unknown }) => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null

  constructor(public url: string) {
    FakeWebSocket.instances.push(this)
  }

  send(data: string) {
    this.sent.push(data)
  }

  close() {
    if (this.closed) return
    this.closed = true
    this.readyState = 3
    this.onclose?.()
  }

  open() {
    this.readyState = 1
    this.onopen?.()
  }

  frame(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) })
  }

  audio(bytes: number[]) {
    this.onmessage?.({ data: new Uint8Array(bytes).buffer })
  }
}

interface Heard {
  codecs: string[]
  frames: Float32Array[]
  ended: string[]
  errors: string[]
}

function harness() {
  const heard: Heard = { codecs: [], frames: [], ended: [], errors: [] }
  const listen = new Listen({
    url: 'ws://test/api/v1/calls/call-1/listen',
    createWebSocket: (url) => new FakeWebSocket(url) as unknown as WebSocket,
    handlers: {
      onReady: (codec) => heard.codecs.push(codec),
      onFrame: (samples) => heard.frames.push(samples),
      onEnded: (reason) => heard.ended.push(reason),
      onError: (message) => heard.errors.push(message),
    },
  })
  listen.start()
  const socket = FakeWebSocket.instances[FakeWebSocket.instances.length - 1]
  return { listen, socket, heard }
}

beforeEach(() => {
  FakeWebSocket.instances = []
})

describe('the listen socket', () => {
  // The session cookie authenticates the handshake, so the
  // client sends nothing at all up this socket.
  it('sends no frame of its own', () => {
    const { socket } = harness()
    socket.open()
    socket.frame({ type: 'ready', codec: 'PCMU' })

    expect(socket.sent).toEqual([])
  })

  it('decodes one frame per binary message in the codec of the call', () => {
    const { socket, heard } = harness()
    socket.open()
    socket.frame({ type: 'ready', codec: 'PCMU' })
    socket.audio([0xff, 0xff, 0xff])

    expect(heard.codecs).toEqual(['PCMU'])
    expect(heard.frames).toHaveLength(1)
    expect(Array.from(heard.frames[0])).toEqual([0, 0, 0])
  })

  it('decodes A-law when the call is A-law', () => {
    const { socket, heard } = harness()
    socket.open()
    socket.frame({ type: 'ready', codec: 'PCMA' })
    socket.audio([0xd5])

    expect(heard.codecs).toEqual(['PCMA'])
    expect(Math.abs(heard.frames[0][0])).toBeLessThan(0.001)
  })

  it('ends with the reason and closes the socket', () => {
    const { socket, heard } = harness()
    socket.open()
    socket.frame({ type: 'ready', codec: 'PCMU' })
    socket.frame({ type: 'ended', reason: 'remote_hangup' })

    expect(heard.ended).toEqual(['remote_hangup'])
    expect(socket.closed).toBe(true)
    expect(heard.errors).toEqual([])
  })

  it('reports a call that is not listenable', () => {
    const { socket, heard } = harness()
    socket.open()
    socket.frame({
      type: 'error',
      code: 'not_found',
      message: 'no call of that id is live',
    })

    expect(heard.errors).toEqual(['no call of that id is live'])
  })

  it('stops without an error when the listener leaves', () => {
    const { listen, socket, heard } = harness()
    socket.open()
    socket.frame({ type: 'ready', codec: 'PCMU' })

    listen.stop()

    expect(socket.closed).toBe(true)
    expect(heard.errors).toEqual([])
  })
})

describe('G.711 decoding', () => {
  it('decodes mu-law silence as zero', () => {
    expect(decodeUlaw(0xff)).toBe(0)
    expect(decodeUlaw(0x7f)).toBe(-0)
  })

  it('decodes the mu-law extremes to the full range', () => {
    expect(decodeUlaw(0x00)).toBe(-32124)
    expect(decodeUlaw(0x80)).toBe(32124)
  })

  it('decodes the A-law extremes to the full range', () => {
    expect(decodeAlaw(0xd5)).toBe(-8)
    expect(decodeAlaw(0x2a)).toBe(32256)
  })

  it('answers one sample for each byte', () => {
    expect(decodeG711(new Uint8Array(160), 'PCMU')).toHaveLength(160)
  })
})
