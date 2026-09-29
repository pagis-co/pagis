// The dictation socket contract (ADR-0020): a cookie handshake,
// PCM16 frames while held, `commit` on release, and the transcript
// frames routed to the composer.

import { beforeEach, describe, expect, it, vi } from 'vitest'

import { Dictation, captureMicrophone, pcm16 } from './dictation'

class FakeWebSocket {
  static OPEN = 1
  static instances: FakeWebSocket[] = []
  sent: (string | ArrayBuffer)[] = []
  closed = false
  readyState = 0
  binaryType = 'blob'
  onopen: (() => void) | null = null
  onmessage: ((event: { data: string }) => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null

  constructor(public url: string) {
    FakeWebSocket.instances.push(this)
  }

  send(data: string | ArrayBuffer) {
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
}

interface Capture {
  frames: ((pcm16: ArrayBuffer) => void)[]
  stopped: number
}

function harness(options: { failCapture?: boolean } = {}) {
  const capture: Capture = { frames: [], stopped: 0 }
  const events: string[] = []
  const dictation = new Dictation({
    url: 'ws://test/api/v1/channels/ch-1/dictate',
    createWebSocket: (url) => new FakeWebSocket(url) as unknown as WebSocket,
    capture: async (onFrame) => {
      if (options.failCapture === true) throw new Error('microphone unavailable')
      capture.frames.push(onFrame)
      return () => {
        capture.stopped += 1
      }
    },
    handlers: {
      onReady: (live) => events.push(`ready:${live}`),
      onDelta: (text) => events.push(`delta:${text}`),
      onFinal: (text) => events.push(`final:${text}`),
      onError: (message) => events.push(`error:${message}`),
    },
  })
  return {
    dictation,
    capture,
    events,
    ws: () => FakeWebSocket.instances[0],
  }
}

async function flush() {
  await new Promise((resolve) => setTimeout(resolve, 0))
}

beforeEach(() => {
  FakeWebSocket.instances = []
  vi.stubGlobal('WebSocket', FakeWebSocket)
})

describe('Dictation', () => {
  it('streams audio frames and commits on release', async () => {
    const h = harness()
    h.dictation.start()
    expect(h.ws().url).toBe('ws://test/api/v1/channels/ch-1/dictate')
    expect(h.ws().binaryType).toBe('arraybuffer')

    h.ws().open()
    await flush()
    // The session cookie authenticates the handshake: the
    // client sends no auth frame, only audio.
    h.ws().frame({ type: 'ready', live: true })

    const frame = new ArrayBuffer(4)
    h.capture.frames[0](frame)
    expect(h.ws().sent[0]).toBe(frame)

    h.ws().frame({ type: 'transcript.delta', text: 'book' })
    h.dictation.release()
    expect(h.capture.stopped).toBe(1)
    expect(h.ws().sent[1]).toBe(JSON.stringify({ type: 'commit' }))

    h.ws().frame({ type: 'transcript.final', text: 'book the room' })
    expect(h.events).toEqual(['ready:true', 'delta:book', 'final:book the room'])
    expect(h.ws().closed).toBe(true)
  })

  it('reports the daemon error and stops the microphone', async () => {
    const h = harness()
    h.dictation.start()
    h.ws().open()
    await flush()

    h.ws().frame({ type: 'error', code: 'transcription_failed', message: 'provider down' })

    expect(h.events).toEqual(['error:provider down'])
    expect(h.capture.stopped).toBe(1)
    expect(h.ws().closed).toBe(true)
  })

  it('reports a lost connection once and never after the final', async () => {
    const h = harness()
    h.dictation.start()
    h.ws().open()
    await flush()

    h.ws().close()
    expect(h.events).toEqual(['error:dictation connection closed'])

    const other = harness()
    other.dictation.start()
    const socket = FakeWebSocket.instances[1]
    socket.open()
    await flush()
    socket.frame({ type: 'transcript.final', text: 'done' })
    socket.close()
    expect(other.events).toEqual(['final:done'])
  })

  it('cancel drops the utterance without a commit', async () => {
    const h = harness()
    h.dictation.start()
    h.ws().open()
    await flush()

    h.dictation.cancel()

    expect(h.ws().sent).toHaveLength(0)
    expect(h.ws().closed).toBe(true)
    expect(h.capture.stopped).toBe(1)
    expect(h.events).toEqual([])
  })

  it('a microphone that will not open is an error', async () => {
    const h = harness({ failCapture: true })
    h.dictation.start()
    h.ws().open()
    await flush()

    expect(h.events).toEqual(['error:microphone unavailable'])
    expect(h.ws().closed).toBe(true)
  })

  it('a release before the socket opened has no transcript to wait for', () => {
    const h = harness()
    h.dictation.start()

    h.dictation.release()

    expect(h.ws().closed).toBe(true)
    expect(h.ws().sent).toEqual([])
  })
})

describe('pcm16', () => {
  it('scales floats to little-endian signed 16-bit and clamps', () => {
    const bytes = new Int16Array(pcm16(new Float32Array([0, 1, -1, 0.5, 2, -2])))
    expect(Array.from(bytes)).toEqual([0, 32767, -32768, 16383, 32767, -32768])
  })
})

// The browser gives the microphone to a secure page alone, so an
// http:// page on another machine has no navigator.mediaDevices.
describe('captureMicrophone', () => {
  it('says that dictation needs a secure address on a page that is not one', async () => {
    const mediaDevices = Object.getOwnPropertyDescriptor(navigator, 'mediaDevices')
    Object.defineProperty(navigator, 'mediaDevices', { value: undefined, configurable: true })
    try {
      await expect(captureMicrophone(() => undefined)).rejects.toThrow(/https:\/\//)
    } finally {
      if (mediaDevices === undefined) {
        Reflect.deleteProperty(navigator, 'mediaDevices')
      } else {
        Object.defineProperty(navigator, 'mediaDevices', mediaDevices)
      }
    }
  })
})
