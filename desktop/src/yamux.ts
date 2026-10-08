// The side of yamux that the Client App needs on its exit socket and on
// its session socket.
//
// The daemon runs the Rust `yamux` crate in client mode over each socket.
// It opens one stream for each connection of a Computer on the exit
// socket, and one for each Coding Session on the session socket. This side
// accepts those streams and opens none. The frame format and the rules are
// the specification of HashiCorp yamux
// (https://github.com/hashicorp/yamux/blob/master/spec.md).
//
// The transport is a seam: the session sends bytes through a function and
// receives the bytes that the caller gives it, so a test drives it with
// raw frames and a socket drives it with WebSocket messages. The frame
// boundaries of the transport mean nothing.
//
// This module uses only erasable TypeScript syntax and imports only Node
// built-ins, so plain `node` loads it in the interop tests of the daemon.

import { Duplex } from 'node:stream'

/** The window that each side of a stream starts with, in bytes. */
export const INITIAL_WINDOW = 256 * 1024

const VERSION = 0
const HEADER_LENGTH = 12

// Frame types.
const DATA = 0
const WINDOW_UPDATE = 1
const PING = 2
const GO_AWAY = 3

// Flags.
const SYN = 0x1
const ACK = 0x2
const FIN = 0x4
const RST = 0x8

// Go Away codes.
const NORMAL = 0
const PROTOCOL_ERROR = 1

/** The largest body of a Data frame that this side sends. The Rust peer
 *  splits its own data the same way. */
const MAX_DATA = 16 * 1024

/** The most streams that can be open at a time. The peer gets RST for a
 *  stream past this count. */
const MAX_STREAMS = 256

interface Header {
  version: number
  type: number
  flags: number
  streamId: number
  length: number
}

function encode(type: number, flags: number, streamId: number, length: number, body?: Uint8Array): Buffer {
  const frame = Buffer.allocUnsafe(HEADER_LENGTH + (body?.length ?? 0))
  frame.writeUInt8(VERSION, 0)
  frame.writeUInt8(type, 1)
  frame.writeUInt16BE(flags, 2)
  frame.writeUInt32BE(streamId, 4)
  frame.writeUInt32BE(length, 8)
  if (body !== undefined) frame.set(body, HEADER_LENGTH)
  return frame
}

function decode(bytes: Buffer): Header {
  return {
    version: bytes.readUInt8(0),
    type: bytes.readUInt8(1),
    flags: bytes.readUInt16BE(2),
    streamId: bytes.readUInt32BE(4),
    length: bytes.readUInt32BE(8),
  }
}

/** What a stream uses of its session. */
interface StreamLink {
  /** Send one frame of the stream. It sends nothing after the session ended. */
  send(type: number, flags: number, length: number, body?: Uint8Array): void
  /** Take the stream out of the session. */
  forget(): void
}

interface PendingWrite {
  bytes: Buffer
  offset: number
  callback: (error?: Error | null) => void
}

/**
 * One stream that the peer opened, as a Duplex.
 *
 * The readable side gets the bytes of the peer, and the peer gets credit
 * only for the bytes that the consumer took out of it. So at most one
 * window of the stream waits in memory, whatever the consumer does.
 */
class YamuxStream extends Duplex {
  private readonly link: StreamLink
  /** The bytes that this side can send before the next Window Update. */
  private sendWindow = INITIAL_WINDOW
  /** The bytes that the peer can send before the next Window Update. */
  receiveWindow = INITIAL_WINDOW
  /** True when the peer sent FIN. */
  finReceived = false
  private finSent = false
  /** True when the stream left the session: both sides sent FIN, one
   *  side reset it, or the session ended. */
  private gone = false
  private pending: PendingWrite | null = null

  constructor(link: StreamLink) {
    // Node.js calls `_read` again when the consumer took the readable
    // side below a quarter of a window. A peer that stalled sent a full
    // window, so by then the consumer took more than half of it, and the
    // credit goes back to the peer at once.
    super({ allowHalfOpen: true, readableHighWaterMark: INITIAL_WINDOW / 4 })
    this.link = link
  }

  /** Bytes from the peer. The session checked them against the window. */
  receiveData(body: Buffer): void {
    this.receiveWindow -= body.length
    this.push(body)
    this.credit()
  }

  /** The peer gave this side more window. */
  receiveWindowUpdate(delta: number): void {
    this.sendWindow += delta
    this.flush()
  }

  receiveFin(): void {
    if (this.finReceived) return
    this.finReceived = true
    this.push(null)
    if (this.finSent) this.leave()
  }

  receiveReset(): void {
    this.leave()
    this.destroy(new Error('the peer reset the stream'))
  }

  /** The session ended, so the stream ends with it and sends nothing. */
  endWith(reason: Error): void {
    this.gone = true
    this.destroy(reason)
  }

  override _read(): void {
    // A read calls `_read` before it takes its bytes out of the buffer, so
    // the credit counts them once the read is complete.
    process.nextTick(() => this.credit())
  }

  override _write(chunk: Buffer, _encoding: BufferEncoding, callback: (error?: Error | null) => void): void {
    this.pending = { bytes: chunk, offset: 0, callback }
    this.flush()
  }

  override _final(callback: (error?: Error | null) => void): void {
    this.finSent = true
    this.link.send(DATA, FIN, 0)
    if (this.finReceived) this.leave()
    callback()
  }

  override _destroy(error: Error | null, callback: (error?: Error | null) => void): void {
    if (!this.gone) {
      this.link.send(DATA, RST, 0)
      this.leave()
    }
    const pending = this.pending
    this.pending = null
    pending?.callback(error ?? new Error('the stream closed before all of its bytes went out'))
    callback(error)
  }

  /**
   * Give the peer credit for the bytes that the consumer took: the bytes
   * that the peer sent past its last credit and that no longer wait in
   * the readable side. The credit goes out when it reaches half a window,
   * so the peer never stalls on a window that the consumer emptied.
   */
  private credit(): void {
    if (this.gone || this.finReceived) return
    const taken = INITIAL_WINDOW - this.receiveWindow - this.readableLength
    if (taken < INITIAL_WINDOW / 2) return
    this.receiveWindow += taken
    this.link.send(WINDOW_UPDATE, 0, taken)
  }

  /** Send as much of the pending write as the window lets through, and
   *  finish the write when all of it went out. */
  private flush(): void {
    const pending = this.pending
    if (pending === null) return
    while (pending.offset < pending.bytes.length && this.sendWindow > 0) {
      const size = Math.min(this.sendWindow, MAX_DATA, pending.bytes.length - pending.offset)
      this.link.send(DATA, 0, size, pending.bytes.subarray(pending.offset, pending.offset + size))
      pending.offset += size
      this.sendWindow -= size
    }
    if (pending.offset < pending.bytes.length) return
    this.pending = null
    pending.callback()
  }

  private leave(): void {
    if (this.gone) return
    this.gone = true
    this.link.forget()
  }
}

export interface YamuxSessionOptions {
  /** Send bytes to the peer, in order. */
  send: (bytes: Uint8Array) => void
  /** Take a stream that the peer opened. */
  onStream: (stream: Duplex) => void
  /** Is told when the session ends by itself: the peer sent Go Away, or
   *  the peer broke the protocol. It is not told of `close` or `abort`. */
  onEnd?: (reason: Error) => void
}

/**
 * One yamux session in which the peer opens every stream.
 *
 * Go Away from the peer, a protocol error and the end of the transport
 * each destroy every stream with an error, so that whatever a stream
 * feeds closes too.
 */
export class YamuxSession {
  private readonly options: YamuxSessionOptions
  private readonly streams = new Map<number, YamuxStream>()
  private input: Buffer[] = []
  private inputLength = 0
  /** The header of a Data frame whose body has not all arrived. */
  private header: Header | null = null
  private ended = false

  constructor(options: YamuxSessionOptions) {
    this.options = options
  }

  /** Take bytes from the transport. A frame can arrive in any number of
   *  parts, and one call can hold many frames. */
  receive(bytes: Uint8Array): void {
    if (this.ended || bytes.length === 0) return
    // A copy, so the caller can use its buffer again.
    this.input.push(Buffer.from(bytes))
    this.inputLength += bytes.length
    while (!this.ended) {
      if (this.header === null) {
        if (this.inputLength < HEADER_LENGTH) return
        const header = decode(this.take(HEADER_LENGTH))
        if (header.version !== VERSION) return this.fail(`a frame of version ${header.version}`)
        if (header.type > GO_AWAY) return this.fail(`a frame of the unknown type ${header.type}`)
        // No stream has a window larger than its first one, so a longer
        // frame is an error before its body arrives.
        if (header.type === DATA && header.length > INITIAL_WINDOW) {
          return this.fail(`a Data frame of ${header.length} bytes, past every window`)
        }
        this.header = header
      }
      const bodyLength = this.header.type === DATA ? this.header.length : 0
      if (this.inputLength < bodyLength) return
      const header = this.header
      this.header = null
      this.dispatch(header, this.take(bodyLength))
    }
  }

  /** Send Go Away and end every stream. */
  close(): void {
    if (this.ended) return
    this.sendFrame(GO_AWAY, 0, 0, NORMAL)
    this.end(new Error('the yamux session closed'))
  }

  /** End every stream and send nothing, as when the transport ended. */
  abort(reason: Error = new Error('the transport of the yamux session closed')): void {
    if (this.ended) return
    this.end(reason)
  }

  private take(count: number): Buffer {
    const joined = this.input.length === 1 ? this.input[0] : Buffer.concat(this.input)
    const rest = joined.subarray(count)
    this.input = rest.length > 0 ? [rest] : []
    this.inputLength -= count
    return joined.subarray(0, count)
  }

  private dispatch(header: Header, body: Buffer): void {
    if (header.type === PING) {
      // A Ping ACK answers a Ping of this side, and this side sends none.
      if ((header.flags & SYN) !== 0) this.sendFrame(PING, ACK, 0, header.length)
      return
    }
    if (header.type === GO_AWAY) {
      const reason = new Error(`the peer ended the yamux session with Go Away code ${header.length}`)
      this.end(reason)
      this.options.onEnd?.(reason)
      return
    }
    this.streamFrame(header, body)
  }

  /** A Data or Window Update frame, with its flags. */
  private streamFrame(header: Header, body: Buffer): void {
    const id = header.streamId
    if ((header.flags & SYN) !== 0) {
      if (id % 2 === 0) return this.fail(`a SYN on the even stream id ${id}`)
      if (this.streams.has(id)) return this.fail(`a SYN on the open stream ${id}`)
      if (this.streams.size >= MAX_STREAMS) {
        this.sendFrame(DATA, RST, id, 0)
        return
      }
      this.open(id)
    }
    // A stream that is not here closed, and the peer sent this frame
    // before it knew. Its consumer can also have destroyed it in
    // `onStream`.
    const stream = this.streams.get(id)
    if (stream === undefined) return
    if ((header.flags & RST) !== 0) {
      stream.receiveReset()
      return
    }
    if (header.type === WINDOW_UPDATE) {
      stream.receiveWindowUpdate(header.length)
    } else if (body.length > 0) {
      if (stream.finReceived) return this.fail(`data on stream ${id} after its FIN`)
      if (body.length > stream.receiveWindow) {
        return this.fail(`${body.length} bytes on stream ${id}, past its window of ${stream.receiveWindow}`)
      }
      stream.receiveData(body)
    }
    if ((header.flags & FIN) !== 0) stream.receiveFin()
  }

  private open(id: number): void {
    const stream: YamuxStream = new YamuxStream({
      send: (type, flags, length, body) => this.sendFrame(type, flags, id, length, body),
      forget: () => {
        if (this.streams.get(id) === stream) this.streams.delete(id)
      },
    })
    this.streams.set(id, stream)
    // The peer waits for the ACK of each stream that it opened.
    this.sendFrame(WINDOW_UPDATE, ACK, id, 0)
    this.options.onStream(stream)
  }

  private sendFrame(type: number, flags: number, streamId: number, length: number, body?: Uint8Array): void {
    if (this.ended) return
    this.options.send(encode(type, flags, streamId, length, body))
  }

  private fail(problem: string): void {
    this.sendFrame(GO_AWAY, 0, 0, PROTOCOL_ERROR)
    const reason = new Error(`yamux protocol error: ${problem}`)
    this.end(reason)
    this.options.onEnd?.(reason)
  }

  private end(reason: Error): void {
    this.ended = true
    this.input = []
    this.inputLength = 0
    this.header = null
    const streams = [...this.streams.values()]
    this.streams.clear()
    for (const stream of streams) stream.endWith(reason)
  }
}
