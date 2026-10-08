// A WebSocket of this client that carries one byte stream: the exit
// socket and the session socket of a Host.
//
// This module uses only erasable TypeScript syntax and imports only Node
// built-ins and `./origin`, so plain `node` loads it in the interop tests
// of the daemon.

import { isTrustedServerOrigin } from './origin'

/** What a link uses of its byte socket. The real one is a WebSocket; a
 *  test gives its own. */
export interface ByteSocket {
  send(bytes: Uint8Array): void
  /** The listener gets the bytes of each binary message. */
  onMessage(listener: (bytes: Uint8Array) => void): void
  /** The listener gets the close code of the socket. */
  onClose(listener: (code: number) => void): void
  close(): void
}

/**
 * A [`ByteSocket`] over a real WebSocket to the daemon, on `path`.
 *
 * It authenticates and trusts as the Host socket does: the Session cookie
 * is a header of the handshake and never a part of the URL, and the
 * socket opens only over `wss://`, or over `ws://` on loopback.
 *
 * The socket carries one byte stream in binary messages, and the
 * boundaries of the messages mean nothing. A text message is not part of
 * it, so the socket closes on one.
 */
export async function openByteSocket(
  url: string,
  sessionSecret: string,
  path: string,
  cookieName = 'pagis_session',
): Promise<ByteSocket> {
  const target = new URL(path, url)
  if (!isTrustedServerOrigin(target)) {
    throw new Error(
      `the socket ${path} does not open to ${target.origin}: it opens only over https://, or over http:// on loopback`,
    )
  }
  target.protocol = target.protocol === 'https:' ? 'wss:' : 'ws:'
  const socket = new WebSocket(target, {
    headers: { cookie: `${cookieName}=${sessionSecret}` },
  } as unknown as string[])
  socket.binaryType = 'arraybuffer'
  await new Promise<void>((resolve, reject) => {
    socket.addEventListener('open', () => resolve(), { once: true })
    socket.addEventListener('error', () => reject(new Error(`the socket ${path} did not open`)), { once: true })
  })
  return {
    send: (bytes) => socket.send(bytes),
    onMessage: (listener) =>
      socket.addEventListener('message', (event) => {
        if (typeof event.data === 'string') {
          socket.close()
          return
        }
        listener(new Uint8Array(event.data as ArrayBuffer))
      }),
    onClose: (listener) =>
      socket.addEventListener('close', (event) => listener(event.code), { once: true }),
    close: () => socket.close(),
  }
}
