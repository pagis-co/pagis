// The exit side of a Client App, for the interop test of the daemon.
//
// A Rust test of the daemon runs this file with plain node, 22.18 or
// later, which strips the types, from the root of the repository:
//
//   node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON desktop/test/exit-peer.ts \
//     <origin> <session secret> <host id> <target host:port>
//
// It opens the real exit socket of `src/exit.ts` and carries every stream
// with the real stream handling. Its dial connects every stream to the
// target, which is on loopback, where the real address check refuses it.
// It prints `ready` on its own line once the socket is open. It exits
// with code 0 when the socket closes or when its standard input ends, so
// the test gives it a pipe as standard input. It prints a failure to
// standard error and exits with code 1.

import { registerHooks } from 'node:module'
import path from 'node:path'

// The modules of the client import each other with no extension, as
// TypeScript resolves them. Plain node needs the `.ts`.
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (/^\.\.?\//.test(specifier) && path.extname(specifier) === '') {
      return nextResolve(`${specifier}.ts`, context)
    }
    return nextResolve(specifier, context)
  },
})

function fail(error: unknown): void {
  const message = error instanceof Error ? error.message : String(error)
  process.stderr.write(`exit-peer: ${message}\n`, () => process.exit(1))
}

try {
  const [origin, secret, hostId, target] = process.argv.slice(2)
  if (target === undefined) {
    throw new Error('usage: exit-peer.ts <origin> <session secret> <host id> <target host:port>')
  }
  const { YamuxSession } = await import('../src/yamux.ts')
  const { carry, connectTcp, openExitSocket, parseDestination } = await import('../src/exit.ts')
  const destination = parseDestination(target)

  const socket = await openExitSocket(origin, secret, hostId)
  const session = new YamuxSession({
    send: (bytes) => socket.send(bytes),
    onStream: (stream) =>
      void carry(stream, (_asked, signal) => connectTcp(destination.host, destination.port, signal)),
    // The daemon broke the protocol or sent Go Away: the interop failed.
    onEnd: (reason) => fail(reason),
  })
  socket.onMessage((bytes) => session.receive(bytes))
  socket.onClose(() => {
    session.abort()
    process.exit(0)
  })
  process.stdin.on('end', () => process.exit(0))
  process.stdin.resume()
  process.stdout.write('ready\n')
} catch (error) {
  fail(error)
}
