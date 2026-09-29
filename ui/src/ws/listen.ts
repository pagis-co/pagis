// The listen-live socket (ADR-0020): the browser opens
// `WS /api/v1/calls/{id}/listen`, which the session cookie
// authenticates, and hears the call from that moment. The daemon answers `ready` with
// the codec of the call, then one binary message for each 20 ms G.711
// frame, and `ended` when the call ends. Nothing goes up this socket:
// control is its own surface.
//
// The frames decode here, because G.711 is not a format the browser
// decodes itself. One byte is one sample at 8 kHz.

export type Law = 'PCMU' | 'PCMA'

export interface ListenHandlers {
  /** The daemon carries the call: audio follows in this law. */
  onReady: (codec: Law) => void
  /** One 20 ms frame as samples in [-1, 1). */
  onFrame: (samples: Float32Array) => void
  onEnded: (reason: string) => void
  onError: (message: string) => void
}

export interface ListenOptions {
  url: string
  handlers: ListenHandlers
  /** Injection point for tests; defaults to the native WebSocket. */
  createWebSocket?: (url: string) => WebSocket
}

/** Samples per second in both laws. */
export const SAMPLE_RATE = 8000
/** The samples in one 20 ms frame. */
export const FRAME_SAMPLES = 160

export class Listen {
  private socket: WebSocket | null = null
  private law: Law = 'PCMU'
  private done = false

  constructor(private readonly options: ListenOptions) {}

  /** Open the socket and start hearing the call. */
  start(): void {
    const create =
      this.options.createWebSocket ?? ((url: string) => new WebSocket(url))
    const socket = create(this.options.url)
    socket.binaryType = 'arraybuffer'
    this.socket = socket
    socket.onmessage = (event: MessageEvent) => this.receive(event.data)
    socket.onerror = () => this.fail('the listen connection failed')
    socket.onclose = () => {
      if (!this.done) this.fail('the listen connection closed')
    }
  }

  /** Stop listening. The call is not told: nothing is signalled. */
  stop(): void {
    this.finish()
  }

  private receive(data: unknown): void {
    if (data instanceof ArrayBuffer) {
      this.options.handlers.onFrame(decodeG711(new Uint8Array(data), this.law))
      return
    }
    let frame: { type?: string; codec?: string; reason?: string; message?: string }
    try {
      frame = JSON.parse(String(data)) as typeof frame
    } catch {
      return
    }
    switch (frame.type) {
      case 'ready':
        this.law = frame.codec === 'PCMA' ? 'PCMA' : 'PCMU'
        this.options.handlers.onReady(this.law)
        break
      case 'ended':
        this.done = true
        this.finish()
        this.options.handlers.onEnded(frame.reason ?? 'unknown')
        break
      case 'error':
        this.fail(frame.message ?? 'the call is not listenable')
        break
      default:
        break
    }
  }

  private fail(message: string): void {
    if (this.done) return
    this.done = true
    this.finish()
    this.options.handlers.onError(message)
  }

  private finish(): void {
    this.done = true
    const socket = this.socket
    this.socket = null
    if (socket !== null) {
      socket.onclose = null
      socket.onerror = null
      socket.close()
    }
  }
}

/** One G.711 frame as samples in [-1, 1). */
export function decodeG711(payload: Uint8Array, law: Law): Float32Array {
  const decode = law === 'PCMA' ? decodeAlaw : decodeUlaw
  const samples = new Float32Array(payload.length)
  for (let i = 0; i < payload.length; i += 1) {
    samples[i] = decode(payload[i]) / 32768
  }
  return samples
}

/** ITU-T G.711 mu-law, one byte to one 14-bit sample scaled to 16 bits. */
export function decodeUlaw(byte: number): number {
  const inverted = ~byte & 0xff
  const sign = inverted & 0x80
  const exponent = (inverted >> 4) & 0x07
  const mantissa = inverted & 0x0f
  let magnitude = ((mantissa << 1) + 33) << exponent
  magnitude -= 33
  magnitude <<= 2
  return sign !== 0 ? -magnitude : magnitude
}

/** ITU-T G.711 A-law, one byte to one 13-bit sample scaled to 16 bits. */
export function decodeAlaw(byte: number): number {
  const toggled = byte ^ 0x55
  const sign = toggled & 0x80
  const exponent = (toggled >> 4) & 0x07
  const mantissa = toggled & 0x0f
  const magnitude =
    exponent === 0
      ? (mantissa << 4) + 8
      : ((mantissa << 4) + 0x108) << (exponent - 1)
  return sign !== 0 ? -magnitude : magnitude
}

/** Where the decoded frames play. */
export interface Speaker {
  play: (samples: Float32Array) => void
  close: () => void
}

/** The speakers, through Web Audio at the call's rate. Each frame is
 *  queued after the one before it, so the call plays without gaps. */
export function webAudioSpeaker(): Speaker {
  const context = new AudioContext({ sampleRate: SAMPLE_RATE })
  let playAt = 0
  return {
    play: (samples) => {
      const buffer = context.createBuffer(1, samples.length, SAMPLE_RATE)
      buffer.getChannelData(0).set(samples)
      const source = context.createBufferSource()
      source.buffer = buffer
      source.connect(context.destination)
      // A listener who falls behind hears the call from now on.
      playAt = Math.max(playAt, context.currentTime)
      source.start(playAt)
      playAt += samples.length / SAMPLE_RATE
    },
    close: () => {
      void context.close()
    },
  }
}
