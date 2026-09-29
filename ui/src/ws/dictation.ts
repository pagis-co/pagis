// The dictation socket (ADR-0020): one utterance per connection.
// The browser opens `WS /api/v1/channels/{id}/dictate`, which the
// session cookie authenticates, sends PCM16 frames while the
// button is held, and sends `commit` on release. The daemon answers
// `ready` (with whether text arrives live), zero or more
// `transcript.delta` frames,
// and one `transcript.final`, then closes. The transcript is a draft:
// nothing here sends a message.

export interface DictationHandlers {
  /** The daemon accepted the utterance; `live` says whether deltas
   *  arrive while the user speaks. */
  onReady: (live: boolean) => void
  onDelta: (text: string) => void
  onFinal: (text: string) => void
  onError: (message: string) => void
}

/** Start capturing microphone audio as PCM16 frames; resolve with the
 *  function that stops it. */
export type AudioCapture = (
  onFrame: (pcm16: ArrayBuffer) => void,
) => Promise<() => void>

export interface DictationOptions {
  url: string
  handlers: DictationHandlers
  /** Injection point for tests; defaults to the native WebSocket. */
  createWebSocket?: (url: string) => WebSocket
  /** Injection point for tests; defaults to the microphone. */
  capture?: AudioCapture
}

/** The sample rate the daemon takes (`pagis_voice::SAMPLE_RATE`). */
export const SAMPLE_RATE = 24_000

export class Dictation {
  private socket: WebSocket | null = null
  private stopCapture: (() => void) | null = null
  private done = false

  constructor(private readonly options: DictationOptions) {}

  /** Open the socket and start the microphone. */
  start(): void {
    const create =
      this.options.createWebSocket ?? ((url: string) => new WebSocket(url))
    const socket = create(this.options.url)
    socket.binaryType = 'arraybuffer'
    this.socket = socket
    socket.onopen = () => {
      const capture = this.options.capture ?? captureMicrophone
      capture((frame) => {
        if (this.socket === socket && socket.readyState === WebSocket.OPEN) {
          socket.send(frame)
        }
      })
        .then((stop) => {
          // Released before the microphone opened: stop it at once.
          if (this.socket !== socket) stop()
          else this.stopCapture = stop
        })
        .catch((error: unknown) => {
          this.fail(error instanceof Error ? error.message : 'microphone unavailable')
        })
    }
    socket.onmessage = (event) => this.receive(String(event.data))
    socket.onerror = () => this.fail('dictation connection failed')
    socket.onclose = () => {
      if (!this.done) this.fail('dictation connection closed')
    }
  }

  /** The button came up: the utterance is over. The socket stays open
   *  for the final transcript. */
  release(): void {
    this.stopMicrophone()
    const socket = this.socket
    if (socket !== null && socket.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify({ type: 'commit' }))
    } else {
      // Nothing reached the daemon: there is no transcript to wait for.
      this.finish()
    }
  }

  /** Drop the utterance without a transcript. */
  cancel(): void {
    this.stopMicrophone()
    this.finish()
  }

  private receive(data: string): void {
    let frame: { type?: string; live?: boolean; text?: string; message?: string }
    try {
      frame = JSON.parse(data) as typeof frame
    } catch {
      return
    }
    switch (frame.type) {
      case 'ready':
        this.options.handlers.onReady(frame.live === true)
        break
      case 'transcript.delta':
        this.options.handlers.onDelta(frame.text ?? '')
        break
      case 'transcript.final':
        this.done = true
        this.finish()
        this.options.handlers.onFinal(frame.text ?? '')
        break
      case 'error':
        this.fail(frame.message ?? 'dictation failed')
        break
      default:
        break
    }
  }

  private fail(message: string): void {
    if (this.done) return
    this.done = true
    this.stopMicrophone()
    this.finish()
    this.options.handlers.onError(message)
  }

  private stopMicrophone(): void {
    this.stopCapture?.()
    this.stopCapture = null
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

/** The microphone as 24 kHz PCM16 frames, through a ScriptProcessorNode
 *  on an AudioContext at the daemon's rate (the browser resamples). The
 *  browser gives the microphone to a secure page alone: an https://
 *  address, or this computer's own. */
export const captureMicrophone: AudioCapture = async (onFrame) => {
  if (navigator.mediaDevices === undefined) {
    throw new Error(
      'Dictation needs the microphone, which the browser gives only to a page at an https:// address or on this computer.',
    )
  }
  const stream = await navigator.mediaDevices.getUserMedia({ audio: true })
  const context = new AudioContext({ sampleRate: SAMPLE_RATE })
  const source = context.createMediaStreamSource(stream)
  const processor = context.createScriptProcessor(4096, 1, 1)
  processor.onaudioprocess = (event) => {
    onFrame(pcm16(event.inputBuffer.getChannelData(0)))
  }
  source.connect(processor)
  processor.connect(context.destination)
  return () => {
    processor.disconnect()
    source.disconnect()
    for (const track of stream.getTracks()) track.stop()
    void context.close()
  }
}

/** Float samples in [-1, 1] as little-endian signed 16-bit. */
export function pcm16(samples: Float32Array): ArrayBuffer {
  const view = new DataView(new ArrayBuffer(samples.length * 2))
  for (let i = 0; i < samples.length; i += 1) {
    const clamped = Math.max(-1, Math.min(1, samples[i]))
    view.setInt16(i * 2, clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff, true)
  }
  return view.buffer
}
