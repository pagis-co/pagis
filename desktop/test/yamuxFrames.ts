// yamux frames as the daemon writes them, and the frames that the client
// sent, for the tests of the links that run yamux.

export const DATA = 0
export const WINDOW_UPDATE = 1
export const GO_AWAY = 3
export const SYN = 0x1
export const FIN = 0x4

export function yamuxFrame(type: number, flags: number, streamId: number, length: number, body = Buffer.alloc(0)): Buffer {
  const header = Buffer.alloc(12)
  header.writeUInt8(type, 1)
  header.writeUInt16BE(flags, 2)
  header.writeUInt32BE(streamId, 4)
  header.writeUInt32BE(length, 8)
  return Buffer.concat([header, body])
}

/** The frame that opens stream `streamId` with `bytes`: a Data frame with
 *  the SYN flag. */
export function openStream(streamId: number, bytes: string): Buffer {
  return yamuxFrame(DATA, SYN, streamId, Buffer.byteLength(bytes), Buffer.from(bytes))
}

/** The yamux frames in bytes that the client sent. */
export function yamuxFrames(bytes: Buffer): Array<{ type: number; flags: number; streamId: number; length: number; body: Buffer }> {
  const frames = []
  let offset = 0
  while (offset + 12 <= bytes.length) {
    const type = bytes.readUInt8(offset + 1)
    const length = bytes.readUInt32BE(offset + 8)
    const bodyLength = type === DATA ? length : 0
    frames.push({
      type,
      flags: bytes.readUInt16BE(offset + 2),
      streamId: bytes.readUInt32BE(offset + 4),
      length,
      body: bytes.subarray(offset + 12, offset + 12 + bodyLength),
    })
    offset += 12 + bodyLength
  }
  return frames
}

/** The bytes that the client sent on one yamux stream. */
export function streamBytes(bytes: Buffer, streamId: number): string {
  return Buffer.concat(
    yamuxFrames(bytes)
      .filter((frame) => frame.type === DATA && frame.streamId === streamId)
      .map((frame) => frame.body),
  ).toString()
}
