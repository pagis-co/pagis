// The session side of a Client App, for the interop test of the daemon.
//
// A Rust test of the daemon runs this file with plain node, 22.18 or
// later, which strips the types, from the root of the repository:
//
//   node --disable-warning=MODULE_TYPELESS_PACKAGE_JSON desktop/test/session-peer.ts \
//     <origin> <session secret>
//
// It opens a Host socket with the global WebSocket, and registers the
// machine with the capabilities `shell` and `harness:claude`. It prints
// `ready <host id>` on its own line once the daemon acknowledged the
// registration. Then it runs the real session link of `src/sessions.ts`,
// which starts a process for each stream with the real stream handling,
// and sends each `session_exit` frame on the Host socket. The environment
// of this process stands in for the login-shell environment, so the test
// decides the PATH. It exits with code 0 when the Host socket closes or
// when its standard input ends, so the test gives it a pipe as standard
// input. It prints a failure to standard error and exits with code 1.

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
  process.stderr.write(`session-peer: ${message}\n`, () => process.exit(1))
}

try {
  const [origin, secret] = process.argv.slice(2)
  if (secret === undefined) {
    throw new Error('usage: session-peer.ts <origin> <session secret>')
  }
  const { openSessionSocket, SessionLink } = await import('../src/sessions.ts')

  const target = new URL('/api/v1/ws', origin)
  target.protocol = target.protocol === 'https:' ? 'wss:' : 'ws:'
  const host = new WebSocket(target, {
    headers: { cookie: `pagis_session=${secret}` },
  } as unknown as string[])
  await new Promise<void>((resolve, reject) => {
    host.addEventListener('open', () => resolve(), { once: true })
    host.addEventListener('error', () => reject(new Error('the Host socket did not open')), { once: true })
  })
  const hostId = await new Promise<string>((resolve) => {
    host.addEventListener('message', (event) => {
      const frame = JSON.parse(String(event.data)) as { type?: string; payload?: { host_id?: string } }
      if (frame.type === 'host.registered' && typeof frame.payload?.host_id === 'string') {
        resolve(frame.payload.host_id)
      }
    })
    host.send(JSON.stringify({ type: 'auth' }))
    host.send(
      JSON.stringify({
        type: 'register_host',
        name: 'session-peer',
        platform: process.platform,
        capabilities: ['shell', 'harness:claude'],
      }),
    )
  })

  const environment: Record<string, string> = {}
  for (const [name, value] of Object.entries(process.env)) {
    if (value !== undefined) environment[name] = value
  }
  const link = new SessionLink(() => openSessionSocket(origin, secret, hostId), {
    send: (frame) => host.send(JSON.stringify(frame)),
    environment: async () => environment,
  })
  link.start()
  host.addEventListener('close', () => void link.stop().then(() => process.exit(0)))
  process.stdin.on('end', () => void link.stop().then(() => process.exit(0)))
  process.stdin.resume()
  process.stdout.write(`ready ${hostId}\n`)
} catch (error) {
  fail(error)
}
