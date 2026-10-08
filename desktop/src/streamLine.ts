// The first line of a stream that the daemon opened.
//
// The daemon starts each stream of the exit socket and of the session
// socket with one line, and the bytes after it belong to what the stream
// carries. So the line is read with a limit and a deadline, and the bytes
// after its line feed go back into the stream.
//
// This module uses only erasable TypeScript syntax and imports only Node
// built-ins, so plain `node` loads it in the interop tests of the daemon.

import type { Duplex } from 'node:stream'

/**
 * The first line of `stream`, without its line feed, as UTF-8. The line
 * is at most `limit` bytes with its line feed. The bytes after the line
 * feed stay in the stream.
 *
 * It rejects when the line is longer, and when the stream ends or closes
 * before the line feed. A line that does not arrive in `timeoutMs`
 * destroys the stream.
 */
export function readLine(stream: Duplex, limit: number, timeoutMs: number): Promise<string> {
  return new Promise((resolve, reject) => {
    let line = Buffer.alloc(0)
    const done = (): void => {
      clearTimeout(timer)
      stream.off('readable', onReadable)
      stream.off('end', onEnd)
      stream.off('close', onClose)
    }
    const timer = setTimeout(() => {
      done()
      const error = new Error(`the first line did not arrive in ${timeoutMs / 1000} s`)
      stream.destroy(error)
      reject(error)
    }, timeoutMs)
    const onReadable = (): void => {
      for (let chunk: Buffer | null = stream.read(); chunk !== null; chunk = stream.read()) {
        const newline = chunk.indexOf(0x0a)
        line = Buffer.concat([line, newline === -1 ? chunk : chunk.subarray(0, newline)])
        if (line.length >= limit) {
          done()
          reject(new Error(`the first line is longer than ${limit - 1} bytes`))
          return
        }
        if (newline !== -1) {
          done()
          // Remove the 'readable' listener before the rest goes back, as
          // the Node.js documentation of `unshift` says.
          if (newline + 1 < chunk.length) stream.unshift(chunk.subarray(newline + 1))
          resolve(line.toString('utf8'))
          return
        }
      }
    }
    const onEnd = (): void => {
      done()
      reject(new Error('the stream ended before its first line'))
    }
    const onClose = (): void => {
      done()
      reject(new Error('the stream closed before its first line'))
    }
    stream.on('readable', onReadable)
    stream.on('end', onEnd)
    stream.on('close', onClose)
  })
}
