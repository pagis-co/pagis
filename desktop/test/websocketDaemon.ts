// The daemon's end of a byte socket, for the tests of the client: a
// server on loopback that accepts every WebSocket upgrade and reads and
// writes the frames of the WebSocket protocol itself.

import { createHash } from 'node:crypto'
import http, { type IncomingHttpHeaders } from 'node:http'
import type { AddressInfo } from 'node:net'
import type { Duplex } from 'node:stream'

/** One frame of the WebSocket protocol as a server writes it: final, and
 *  not masked. */
function serverFrame(opcode: number, payload: Buffer): Buffer {
  let head: Buffer
  if (payload.length < 126) {
    head = Buffer.from([0x80 | opcode, payload.length])
  } else if (payload.length < 65_536) {
    head = Buffer.alloc(4)
    head[1] = 126
    head.writeUInt16BE(payload.length, 2)
  } else {
    head = Buffer.alloc(10)
    head[1] = 127
    head.writeBigUInt64BE(BigInt(payload.length), 2)
  }
  head[0] = 0x80 | opcode
  return Buffer.concat([head, payload])
}

export const BINARY = 0x2
export const TEXT = 0x1
export const CLOSE = 0x8

/** The daemon's end of one exit socket, at the level of WebSocket frames. */
export class DaemonEnd {
  readonly url: string
  readonly headers: IncomingHttpHeaders
  readonly socket: Duplex
  /** The frames that the client sent, unmasked. */
  readonly frames: Array<{ opcode: number; payload: Buffer }> = []
  private input = Buffer.alloc(0)

  constructor(url: string, headers: IncomingHttpHeaders, socket: Duplex) {
    this.url = url
    this.headers = headers
    this.socket = socket
    socket.on('data', (chunk: Buffer) => this.read(chunk))
  }

  binary(bytes: Buffer): void {
    this.socket.write(serverFrame(BINARY, bytes))
  }

  text(text: string): void {
    this.socket.write(serverFrame(TEXT, Buffer.from(text)))
  }

  /** Close with a code, as the daemon does, and end the connection. */
  closeWith(code: number): void {
    const payload = Buffer.alloc(2)
    payload.writeUInt16BE(code)
    this.socket.end(serverFrame(CLOSE, payload))
  }

  /** The bytes of every binary message that the client sent. */
  received(): Buffer {
    return Buffer.concat(this.frames.filter((frame) => frame.opcode === BINARY).map((frame) => frame.payload))
  }

  private read(chunk: Buffer): void {
    this.input = Buffer.concat([this.input, chunk])
    for (;;) {
      if (this.input.length < 2) return
      const opcode = this.input[0] & 0x0f
      let length = this.input[1] & 0x7f
      let offset = 2
      if (length === 126) {
        if (this.input.length < 4) return
        length = this.input.readUInt16BE(2)
        offset = 4
      } else if (length === 127) {
        if (this.input.length < 10) return
        length = Number(this.input.readBigUInt64BE(2))
        offset = 10
      }
      // A client masks every frame.
      if (this.input.length < offset + 4 + length) return
      const mask = this.input.subarray(offset, offset + 4)
      const payload = Buffer.from(this.input.subarray(offset + 4, offset + 4 + length))
      for (let index = 0; index < payload.length; index += 1) payload[index] ^= mask[index % 4]
      this.frames.push({ opcode, payload })
      this.input = this.input.subarray(offset + 4 + length)
    }
  }
}

/** A daemon on loopback that accepts every WebSocket upgrade. */
export async function daemonServer(): Promise<{ origin: string; ends: DaemonEnd[]; close: () => Promise<void> }> {
  const ends: DaemonEnd[] = []
  const server = http.createServer()
  server.on('upgrade', (request, socket) => {
    socket.on('error', () => {})
    const accept = createHash('sha1')
      .update(`${request.headers['sec-websocket-key']}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
      .digest('base64')
    socket.write(
      [
        'HTTP/1.1 101 Switching Protocols',
        'Upgrade: websocket',
        'Connection: Upgrade',
        `Sec-WebSocket-Accept: ${accept}`,
        '',
        '',
      ].join('\r\n'),
    )
    ends.push(new DaemonEnd(request.url ?? '', request.headers, socket))
  })
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  return {
    origin: `http://127.0.0.1:${(server.address() as AddressInfo).port}/`,
    ends,
    close: async () => {
      for (const end of ends) end.socket.destroy()
      await new Promise((resolve) => server.close(resolve))
    },
  }
}
