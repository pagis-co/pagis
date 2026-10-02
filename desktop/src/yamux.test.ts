// The yamux session of the exit socket, driven with raw frames as the
// daemon's Rust yamux in client mode sends them: it opens every stream
// with an odd id, its first frame of a stream is a Data frame with SYN
// that already carries bytes, it sends FIN and RST as Data frames of
// length 0, and it splits data into frames of at most 16 KiB.

import { randomBytes } from 'node:crypto'
import type { Duplex } from 'node:stream'

import { describe, expect, it } from 'vitest'

import { INITIAL_WINDOW, YamuxSession } from './yamux'

const DATA = 0
const WINDOW_UPDATE = 1
const PING = 2
const GO_AWAY = 3

const SYN = 0x1
const ACK = 0x2
const FIN = 0x4
const RST = 0x8

const KIB = 1024

/** One frame as the peer writes it: a 12-byte big-endian header, then
 *  the body of a Data frame. */
function frame(type: number, flags: number, streamId: number, length: number, body = Buffer.alloc(0)): Buffer {
  const header = Buffer.alloc(12)
  header.writeUInt8(0, 0)
  header.writeUInt8(type, 1)
  header.writeUInt16BE(flags, 2)
  header.writeUInt32BE(streamId, 4)
  header.writeUInt32BE(length, 8)
  return Buffer.concat([header, body])
}

function data(streamId: number, body: Buffer | string, flags = 0): Buffer {
  const bytes = Buffer.from(body)
  return frame(DATA, flags, streamId, bytes.length, bytes)
}

interface Frame {
  version: number
  type: number
  flags: number
  streamId: number
  length: number
  body: Buffer
}

/** The frames in the bytes that the session sent. */
function parse(bytes: Buffer): Frame[] {
  const frames: Frame[] = []
  let offset = 0
  while (offset < bytes.length) {
    const type = bytes.readUInt8(offset + 1)
    const length = bytes.readUInt32BE(offset + 8)
    const bodyLength = type === DATA ? length : 0
    frames.push({
      version: bytes.readUInt8(offset),
      type,
      flags: bytes.readUInt16BE(offset + 2),
      streamId: bytes.readUInt32BE(offset + 4),
      length,
      body: bytes.subarray(offset + 12, offset + 12 + bodyLength),
    })
    offset += 12 + bodyLength
  }
  expect(offset, 'the session sent a frame in parts').toBe(bytes.length)
  return frames
}

/** The daemon's end of a session: it keeps every byte that the session
 *  sent, every stream that the session accepted and why the session
 *  ended. */
class Peer {
  readonly sent: Buffer[] = []
  readonly streams: Duplex[] = []
  readonly errors = new Map<Duplex, Error>()
  readonly ended: Error[] = []
  readonly session = new YamuxSession({
    send: (bytes) => {
      this.sent.push(Buffer.from(bytes))
    },
    onStream: (stream) => {
      stream.on('error', (error: Error) => this.errors.set(stream, error))
      this.streams.push(stream)
    },
    onEnd: (reason) => {
      this.ended.push(reason)
    },
  })

  receive(...frames: Buffer[]): void {
    for (const bytes of frames) this.session.receive(bytes)
  }

  frames(): Frame[] {
    return parse(Buffer.concat(this.sent))
  }

  /** The bytes of the Data frames that the session sent on a stream. */
  bodyOf(streamId: number): Buffer {
    return Buffer.concat(
      this.frames()
        .filter((sent) => sent.type === DATA && sent.streamId === streamId)
        .map((sent) => sent.body),
    )
  }

  /** The credit that the session gave the peer on a stream. */
  creditOf(streamId: number): number {
    return this.frames()
      .filter((sent) => sent.type === WINDOW_UPDATE && sent.streamId === streamId)
      .reduce((total, sent) => total + sent.length, 0)
  }

  /** The frames other than Data and Window Update frames of streams. */
  controlFrames(): Frame[] {
    return this.frames().filter((sent) => sent.type === PING || sent.type === GO_AWAY)
  }
}

/** Let the streams run their queued events. */
async function settle(): Promise<void> {
  for (let turn = 0; turn < 5; turn += 1) await new Promise((resolve) => setImmediate(resolve))
}

/** Read everything a stream delivers until it ends. */
function readAll(stream: Duplex): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = []
    stream.on('data', (chunk: Buffer) => chunks.push(chunk))
    stream.once('end', () => resolve(Buffer.concat(chunks)))
    stream.once('error', reject)
  })
}

const EMPTY = Buffer.alloc(0)

describe('the frames of a session', () => {
  /** Two streams opened with their preambles, a Ping, more data, a
   *  Window Update and a FIN. */
  function conversation(): Buffer {
    return Buffer.concat([
      data(1, 'example.com:443\n', SYN),
      frame(PING, SYN, 0, 7),
      data(3, 'pagis.example.com:80\n', SYN),
      data(1, 'hello'),
      frame(WINDOW_UPDATE, 0, 1, 1000),
      data(1, '', FIN),
      data(3, 'GET / HTTP/1.1\r\n'),
      data(3, '', FIN),
    ])
  }

  async function outcome(feed: (peer: Peer) => void): Promise<{ sent: string; delivered: string[] }> {
    const peer = new Peer()
    feed(peer)
    const delivered = await Promise.all(peer.streams.map(readAll))
    return { sent: Buffer.concat(peer.sent).toString('hex'), delivered: delivered.map((bytes) => bytes.toString()) }
  }

  it('reads the same frames whatever the split of the input', async () => {
    const input = conversation()
    const whole = await outcome((peer) => peer.receive(input))
    expect(whole.delivered).toEqual(['example.com:443\nhello', 'pagis.example.com:80\nGET / HTTP/1.1\r\n'])

    for (let split = 0; split <= input.length; split += 1) {
      const result = await outcome((peer) => peer.receive(input.subarray(0, split), input.subarray(split)))
      expect(result, `a split at byte ${split}`).toEqual(whole)
    }
    const byteByByte = await outcome((peer) => {
      for (let index = 0; index < input.length; index += 1) peer.receive(input.subarray(index, index + 1))
    })
    expect(byteByByte).toEqual(whole)
  })

  /** The caller can use its buffer again after `receive`. */
  it('keeps no reference to the bytes it was given', () => {
    const peer = new Peer()
    const input = Buffer.from(data(1, 'example.com:443\n', SYN))
    peer.receive(input.subarray(0, 20))
    const rest = Buffer.from(input.subarray(20))
    peer.receive(rest)
    input.fill(0)
    rest.fill(0)

    expect(peer.streams[0].read().toString()).toBe('example.com:443\n')
  })
})

describe('a stream that the peer opens', () => {
  it('opens on a Data frame with SYN, keeps its bytes and is ACKed at once', () => {
    const peer = new Peer()

    peer.receive(data(1, 'example.com:443\n', SYN))

    expect(peer.streams).toHaveLength(1)
    expect(peer.frames()).toEqual([
      { version: 0, type: WINDOW_UPDATE, flags: ACK, streamId: 1, length: 0, body: EMPTY },
    ])
    expect(peer.streams[0].read().toString()).toBe('example.com:443\n')
  })

  it('opens on a Window Update with SYN, and adds its delta to the send window', async () => {
    const peer = new Peer()

    peer.receive(frame(WINDOW_UPDATE, SYN, 1, 64 * KIB))
    peer.streams[0].write(Buffer.alloc(INITIAL_WINDOW + 128 * KIB))
    await settle()

    expect(peer.frames()[0]).toMatchObject({ type: WINDOW_UPDATE, flags: ACK, streamId: 1, length: 0 })
    expect(peer.bodyOf(1).length).toBe(INITIAL_WINDOW + 64 * KIB)
  })

  it('answers a Ping with the same opaque value', () => {
    const peer = new Peer()

    peer.receive(frame(PING, SYN, 0, 0xdeadbeef))
    // A Ping ACK answers a Ping of this side, and this side sends none.
    peer.receive(frame(PING, ACK, 0, 12))

    expect(peer.frames()).toEqual([
      { version: 0, type: PING, flags: ACK, streamId: 0, length: 0xdeadbeef, body: EMPTY },
    ])
  })
})

describe('the windows of a stream', () => {
  /** The peer sends `bytes` on a stream in frames of 16 KiB, and never
   *  more than the window that the session gave it. It answers the
   *  largest count of bytes that it had in flight. */
  async function sendWithinWindow(peer: Peer, streamId: number, bytes: Buffer, already = 0): Promise<number> {
    let sent = already
    let largestInFlight = 0
    for (let turn = 0; sent < already + bytes.length; turn += 1) {
      expect(turn, 'the session gave no more credit').toBeLessThan(10_000)
      const window = INITIAL_WINDOW + peer.creditOf(streamId) - sent
      if (window === 0) {
        await new Promise((resolve) => setTimeout(resolve, 1))
        continue
      }
      const offset = sent - already
      const size = Math.min(16 * KIB, window, bytes.length - offset)
      peer.receive(data(streamId, bytes.subarray(offset, offset + size)))
      sent += size
      largestInFlight = Math.max(largestInFlight, sent - peer.creditOf(streamId))
    }
    return largestInFlight
  }

  /** The peer reads every Data frame that arrives and gives its credit
   *  back, until `done`. */
  async function readWithCredit(peer: Peer, streamId: number, done: () => boolean): Promise<void> {
    let credited = 0
    for (let turn = 0; !done(); turn += 1) {
      expect(turn, 'the session sent nothing more').toBeLessThan(1_000)
      const received = peer.bodyOf(streamId).length
      expect(received - credited, 'the session sent past its window').toBeLessThanOrEqual(INITIAL_WINDOW)
      if (received > credited) peer.receive(frame(WINDOW_UPDATE, 0, streamId, received - credited))
      credited = received
      await settle()
    }
  }

  it('carries more than a window from the peer, in order, to a consumer that reads', async () => {
    const peer = new Peer()
    const payload = randomBytes(1024 * KIB)
    peer.receive(data(1, payload.subarray(0, 100), SYN))
    const delivered = readAll(peer.streams[0])

    const largestInFlight = await sendWithinWindow(peer, 1, payload.subarray(100), 100)
    peer.receive(data(1, '', FIN))

    expect((await delivered).equals(payload)).toBe(true)
    expect(largestInFlight).toBeLessThanOrEqual(INITIAL_WINDOW)
  })

  it('carries more than a window to the peer, in order, in frames of at most 16 KiB', async () => {
    const peer = new Peer()
    const payload = randomBytes(1024 * KIB)
    peer.receive(data(1, 'example.com:443\n', SYN))
    let written = false
    peer.streams[0].write(payload, () => {
      written = true
    })

    await readWithCredit(peer, 1, () => written)

    const sizes = peer.frames().filter((sent) => sent.type === DATA).map((sent) => sent.body.length)
    expect(Math.max(...sizes)).toBe(16 * KIB)
    expect(peer.bodyOf(1).equals(payload)).toBe(true)
  })

  it('sends no more than the send window, and finishes a write only when all of it went out', async () => {
    const peer = new Peer()
    peer.receive(data(1, 'example.com:443\n', SYN))
    let written = false
    peer.streams[0].write(Buffer.alloc(1024 * KIB, 7), () => {
      written = true
    })
    await settle()

    expect(peer.bodyOf(1).length).toBe(INITIAL_WINDOW)
    expect(written).toBe(false)

    peer.receive(frame(WINDOW_UPDATE, 0, 1, 100 * KIB))
    await settle()
    expect(peer.bodyOf(1).length).toBe(INITIAL_WINDOW + 100 * KIB)
    expect(written).toBe(false)

    // The Rust peer tunes its window up past 256 KiB.
    peer.receive(frame(WINDOW_UPDATE, 0, 1, 4096 * KIB))
    await settle()
    expect(peer.bodyOf(1).length).toBe(1024 * KIB)
    expect(written).toBe(true)
  })

  it('stops the credit while the consumer reads nothing, and a read gives it back', async () => {
    const peer = new Peer()
    const payload = randomBytes(INITIAL_WINDOW)
    peer.receive(data(1, payload.subarray(0, 16 * KIB), SYN))
    const stream = peer.streams[0]

    await sendWithinWindow(peer, 1, payload.subarray(16 * KIB), 16 * KIB)
    await settle()

    // Every byte waits in the stream, and the peer has no window left.
    expect(peer.creditOf(1)).toBe(0)
    expect(stream.readableLength).toBe(INITIAL_WINDOW)

    const chunks: Buffer[] = []
    for (let chunk = stream.read(); chunk !== null; chunk = stream.read()) chunks.push(chunk)
    expect(Buffer.concat(chunks).equals(payload)).toBe(true)
    await settle()

    expect(peer.creditOf(1)).toBe(INITIAL_WINDOW)
  })

  it('gives credit only for the bytes that a slow consumer took, and never stalls the peer', async () => {
    const peer = new Peer()
    const payload = randomBytes(1024 * KIB)
    peer.receive(data(1, payload.subarray(0, 100), SYN))
    const stream = peer.streams[0]
    const chunks: Buffer[] = []
    let taken = 0
    stream.on('data', (chunk: Buffer) => {
      chunks.push(chunk)
      taken += chunk.length
      // A consumer that stops after each chunk, as a slow socket does.
      stream.pause()
      setTimeout(() => stream.resume(), 1)
      expect(peer.creditOf(1)).toBeLessThanOrEqual(taken)
    })

    const largestInFlight = await sendWithinWindow(peer, 1, payload.subarray(100), 100)
    peer.receive(data(1, '', FIN))
    await new Promise((resolve) => stream.once('end', resolve))

    expect(Buffer.concat(chunks).equals(payload)).toBe(true)
    expect(largestInFlight).toBeLessThanOrEqual(INITIAL_WINDOW)
  })

  it('ends the session when the peer sends past the receive window', async () => {
    const peer = new Peer()
    peer.receive(data(1, Buffer.alloc(INITIAL_WINDOW), SYN))
    const stream = peer.streams[0]

    peer.receive(data(1, 'x'))
    await settle()

    expect(peer.controlFrames()).toEqual([
      { version: 0, type: GO_AWAY, flags: 0, streamId: 0, length: 1, body: EMPTY },
    ])
    expect(stream.destroyed).toBe(true)
    expect(peer.ended[0].message).toMatch(/window/)
  })
})

describe('the end of a stream', () => {
  it('ends the readable side on a FIN from the peer, and sends FIN when the writable side ends', async () => {
    const peer = new Peer()
    peer.receive(data(1, 'example.com:443\n', SYN))
    const stream = peer.streams[0]
    const delivered = readAll(stream)

    peer.receive(data(1, 'last bytes', FIN))
    expect((await delivered).toString()).toBe('example.com:443\nlast bytes')

    stream.end('answer')
    await settle()

    expect(peer.frames().slice(1)).toEqual([
      { version: 0, type: DATA, flags: 0, streamId: 1, length: 6, body: Buffer.from('answer') },
      { version: 0, type: DATA, flags: FIN, streamId: 1, length: 0, body: EMPTY },
    ])
    // Both sides closed, so the stream closed with no reset.
    expect(stream.destroyed).toBe(true)
    expect(peer.errors.has(stream)).toBe(false)
  })

  it('takes a FIN on a Window Update as well', async () => {
    const peer = new Peer()
    peer.receive(data(1, 'example.com:443\n', SYN))
    const delivered = readAll(peer.streams[0])

    peer.receive(frame(WINDOW_UPDATE, FIN, 1, 0))

    expect((await delivered).toString()).toBe('example.com:443\n')
  })

  it('carries bytes to the peer after the peer half-closed', async () => {
    const peer = new Peer()
    peer.receive(data(1, 'example.com:443\n', SYN), data(1, '', FIN))
    const stream = peer.streams[0]
    stream.resume()

    stream.end('the response')
    await settle()

    expect(peer.bodyOf(1).toString()).toBe('the response')
  })

  it('is destroyed with an error on a RST from the peer', async () => {
    const peer = new Peer()
    peer.receive(data(1, 'example.com:443\n', SYN))
    const stream = peer.streams[0]

    peer.receive(data(1, '', RST))
    await settle()

    expect(stream.destroyed).toBe(true)
    expect(peer.errors.get(stream)?.message).toMatch(/reset/)
  })

  it('fails a write that waits for window when the peer resets the stream', async () => {
    const peer = new Peer()
    peer.receive(data(1, 'example.com:443\n', SYN))
    const stream = peer.streams[0]
    const outcome = new Promise<Error | null | undefined>((resolve) => {
      stream.write(Buffer.alloc(INITIAL_WINDOW + 1), resolve)
    })
    await settle()

    peer.receive(frame(WINDOW_UPDATE, RST, 1, 0))

    expect(await outcome).toBeInstanceOf(Error)
  })

  it('sends RST when the consumer destroys a stream that is open', async () => {
    const peer = new Peer()
    peer.receive(data(1, 'example.com:443\n', SYN))

    peer.streams[0].destroy()
    await settle()

    expect(peer.frames().slice(1)).toEqual([
      { version: 0, type: DATA, flags: RST, streamId: 1, length: 0, body: EMPTY },
    ])
  })

  /** Frames that the peer sent before it read the RST arrive after it. */
  it('ignores the frames of a stream that closed', async () => {
    const peer = new Peer()
    peer.receive(data(1, 'example.com:443\n', SYN))
    peer.streams[0].destroy()
    await settle()
    const before = peer.frames().length

    peer.receive(data(1, 'in flight'), frame(WINDOW_UPDATE, 0, 1, 100), data(1, '', FIN))
    await settle()

    expect(peer.frames()).toHaveLength(before)
    expect(peer.ended).toEqual([])
  })
})

describe('the end of a session', () => {
  async function twoStreams(): Promise<Peer> {
    const peer = new Peer()
    peer.receive(data(1, 'example.com:443\n', SYN), data(3, 'example.org:443\n', SYN))
    await settle()
    return peer
  }

  it('destroys every stream with an error on a Go Away from the peer', async () => {
    const peer = await twoStreams()

    peer.receive(frame(GO_AWAY, 0, 0, 0))
    await settle()

    expect(peer.streams.every((stream) => stream.destroyed && peer.errors.has(stream))).toBe(true)
    expect(peer.ended).toHaveLength(1)
    // The session is over: it answers nothing more.
    peer.receive(frame(PING, SYN, 0, 1))
    expect(peer.controlFrames()).toEqual([])
  })

  it('sends Go Away with the protocol error code and destroys every stream on a protocol error', async () => {
    const peer = await twoStreams()

    // A frame of version 1, which does not exist.
    const unknown = frame(PING, SYN, 0, 1)
    unknown.writeUInt8(1, 0)
    peer.receive(unknown)
    await settle()

    expect(peer.controlFrames()).toEqual([
      { version: 0, type: GO_AWAY, flags: 0, streamId: 0, length: 1, body: EMPTY },
    ])
    expect(peer.streams.every((stream) => stream.destroyed && peer.errors.has(stream))).toBe(true)
    expect(peer.ended[0].message).toMatch(/version/)
  })

  it('is a protocol error to send a frame of an unknown type', () => {
    const peer = new Peer()

    peer.receive(frame(9, 0, 0, 0))

    expect(peer.controlFrames()).toEqual([
      { version: 0, type: GO_AWAY, flags: 0, streamId: 0, length: 1, body: EMPTY },
    ])
  })

  it('is a protocol error to open a stream with an even id', () => {
    const peer = new Peer()

    peer.receive(data(2, 'example.com:443\n', SYN))

    expect(peer.streams).toEqual([])
    expect(peer.controlFrames()).toEqual([
      { version: 0, type: GO_AWAY, flags: 0, streamId: 0, length: 1, body: EMPTY },
    ])
  })

  it('is a protocol error to open a stream with the id of an open stream', async () => {
    const peer = await twoStreams()

    peer.receive(data(1, 'example.net:443\n', SYN))
    await settle()

    expect(peer.streams).toHaveLength(2)
    expect(peer.controlFrames()).toEqual([
      { version: 0, type: GO_AWAY, flags: 0, streamId: 0, length: 1, body: EMPTY },
    ])
    expect(peer.streams.every((stream) => stream.destroyed)).toBe(true)
  })

  it('is a protocol error to send data after a FIN', async () => {
    const peer = new Peer()
    peer.receive(data(1, 'example.com:443\n', SYN), data(1, '', FIN))

    peer.receive(data(1, 'more'))

    expect(peer.controlFrames()).toEqual([
      { version: 0, type: GO_AWAY, flags: 0, streamId: 0, length: 1, body: EMPTY },
    ])
  })

  it('sends Go Away with the normal code and destroys every stream when it closes', async () => {
    const peer = await twoStreams()

    peer.session.close()
    await settle()

    expect(peer.controlFrames()).toEqual([
      { version: 0, type: GO_AWAY, flags: 0, streamId: 0, length: 0, body: EMPTY },
    ])
    expect(peer.streams.every((stream) => stream.destroyed && peer.errors.has(stream))).toBe(true)
    // The owner closed it, so the owner is not told again.
    expect(peer.ended).toEqual([])
  })

  it('sends nothing and destroys every stream when the transport ends', async () => {
    const peer = await twoStreams()
    const before = peer.sent.length

    peer.session.abort()
    await settle()

    expect(peer.sent).toHaveLength(before)
    expect(peer.streams.every((stream) => stream.destroyed && peer.errors.has(stream))).toBe(true)
    expect(peer.ended).toEqual([])
  })
})

describe('the count of open streams', () => {
  function openStreams(peer: Peer, count: number): void {
    for (let index = 0; index < count; index += 1) peer.receive(data(2 * index + 1, 'example.com:443\n', SYN))
  }

  it('refuses the 257th open stream with RST', () => {
    const peer = new Peer()
    openStreams(peer, 256)

    peer.receive(data(513, 'example.com:443\n', SYN))

    expect(peer.streams).toHaveLength(256)
    expect(peer.frames().at(-1)).toEqual({ version: 0, type: DATA, flags: RST, streamId: 513, length: 0, body: EMPTY })
    expect(peer.ended).toEqual([])
  })

  it('accepts a new stream once an open one closed on both sides', async () => {
    const peer = new Peer()
    openStreams(peer, 256)
    const first = peer.streams[0]
    first.resume()
    first.end()
    peer.receive(data(1, '', FIN))
    await settle()

    peer.receive(data(513, 'example.com:443\n', SYN))

    expect(peer.streams).toHaveLength(257)
    expect(peer.frames().at(-1)).toMatchObject({ type: WINDOW_UPDATE, flags: ACK, streamId: 513 })
  })
})
