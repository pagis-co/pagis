// The Coding Sessions of this Host: the harness process that each session
// stream starts, its answer, its stdio, its end, and the link that keeps
// the session socket open. The processes are real child processes of
// `process.execPath`, with a fake login-shell environment. The last case
// runs the interop harness of the daemon's test under plain node.

import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, realpathSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { type Duplex, duplexPair } from 'node:stream'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { daemonServer, TEXT } from '../test/websocketDaemon'
import { DATA, FIN, openStream, streamBytes, yamuxFrame } from '../test/yamuxFrames'
import type { ByteSocket } from './byteSocket'
import { type SessionDeps, type SessionExitFrame, SessionLink, startSession } from './sessions'

/** The command of the process, which the fake PATH holds. */
const NODE = path.basename(process.execPath)

/** The environment of the fake login shell. */
const SHELL_ENVIRONMENT = { PATH: path.dirname(process.execPath), FROM_SHELL: 'shell', BOTH: 'shell' }

/** A process that copies its stdin to its stdout, and so exits at the end
 *  of its input, as an ACP agent does. */
const COPY = 'process.stdin.pipe(process.stdout)'

/** A process that prints its pid and the pid of its own child, then runs
 *  until a signal ends it. */
const WITH_CHILD = [
  "const child = require('node:child_process').spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'])",
  "process.stdout.write(`${process.pid} ${child.pid}\\n`)",
  'setInterval(() => {}, 1000)',
].join(';')

const directories: string[] = []
const pids: number[] = []

function directory(): string {
  const made = realpathSync(mkdtempSync(path.join(os.tmpdir(), 'pagis-sessions-')))
  directories.push(made)
  return made
}

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

afterEach(() => {
  for (const pid of pids.splice(0)) {
    if (isAlive(pid)) process.kill(pid, 'SIGKILL')
  }
})

interface OpenRequestLine {
  session_id: string
  command: string
  args: string[]
  cwd: string
  env: Record<string, string>
  worktree: null | { repo: string; branch: string; base: string }
}

function request(cwd: string, overrides: Partial<OpenRequestLine> = {}): string {
  const line: OpenRequestLine = {
    session_id: 'session-1',
    command: NODE,
    args: ['-e', COPY],
    cwd,
    env: {},
    worktree: null,
    ...overrides,
  }
  return `${JSON.stringify(line)}\n`
}

/** A stream as the daemon opens it: `startSession` gets one end, and the
 *  test is the daemon at the other end. */
function session(deps: Partial<SessionDeps> = {}) {
  const [client, daemon] = duplexPair()
  const chunks: Buffer[] = []
  daemon.on('data', (chunk: Buffer) => chunks.push(chunk))
  daemon.on('error', () => {})
  const exits: SessionExitFrame[] = []
  const done = startSession(client, {
    environment: async () => ({ ...SHELL_ENVIRONMENT }),
    home: os.homedir(),
    send: (frame) => exits.push(frame),
    graceMs: 200,
    ...deps,
  })
  const output = (): string => Buffer.concat(chunks).toString()
  return {
    daemon,
    exits,
    done,
    output,
    /** The answer line, parsed, once it arrived. */
    answer: async (): Promise<Record<string, unknown>> => {
      await vi.waitFor(() => expect(output()).toContain('\n'))
      return JSON.parse(output().slice(0, output().indexOf('\n'))) as Record<string, unknown>
    },
    /** What the process wrote after the answer. */
    stdout: (): string => output().slice(output().indexOf('\n') + 1),
  }
}

/** Wait until the readable side of `side` ended. */
function ended(side: Duplex): Promise<void> {
  return new Promise((resolve) => {
    if (side.readableEnded) resolve()
    else side.once('end', () => resolve())
  })
}

describe('a session stream', () => {
  it('answers ok with the resolved directory, then carries lines both ways through the process', async () => {
    const cwd = directory()
    const { daemon, exits, done, answer, stdout } = session()

    daemon.write(request(cwd))

    expect(await answer()).toEqual({ ok: true, cwd })
    daemon.write('{"jsonrpc":"2.0","id":0,"method":"initialize"}\n')
    await vi.waitFor(() => expect(stdout()).toBe('{"jsonrpc":"2.0","id":0,"method":"initialize"}\n'))
    daemon.write('{"jsonrpc":"2.0","id":1,"method":"session/new"}\n')
    await vi.waitFor(() => expect(stdout()).toContain('"id":1'))

    // The FIN of the daemon ends stdin, and the process exits by itself.
    daemon.end()
    await ended(daemon)
    await done
    expect(exits).toEqual([{ type: 'session_exit', session_id: 'session-1', exit_code: 0, stderr_tail: '' }])
  })

  it('hands the process the bytes after a request line that arrives in parts', async () => {
    const cwd = directory()
    const { daemon, answer, stdout } = session()
    const line = request(cwd)

    daemon.write(line.slice(0, 20))
    await new Promise((resolve) => setTimeout(resolve, 5))
    daemon.write(line.slice(20, -1))
    await new Promise((resolve) => setTimeout(resolve, 5))
    daemon.write('\nthe first bytes\n')

    expect(await answer()).toEqual({ ok: true, cwd })
    await vi.waitFor(() => expect(stdout()).toBe('the first bytes\n'))
    daemon.end()
  })

  it('gives the process the environment of the request over the login-shell environment', async () => {
    const { daemon, answer, stdout } = session()

    daemon.write(
      request(directory(), {
        args: ['-e', 'process.stdout.write(JSON.stringify([process.env.FROM_SHELL, process.env.FROM_DAEMON, process.env.BOTH]))'],
        env: { FROM_DAEMON: 'daemon', BOTH: 'daemon' },
      }),
    )

    expect(await answer()).toMatchObject({ ok: true })
    await vi.waitFor(() => expect(stdout()).not.toBe(''))
    expect(JSON.parse(stdout())).toEqual(['shell', 'daemon', 'daemon'])
  })

  it('resolves ~ and a directory under ~/ against the home directory of the OS user', async () => {
    const home = directory()
    mkdirSync(path.join(home, 'code'))
    const inHome = session({ home })
    const underHome = session({ home })

    inHome.daemon.write(request('~', { args: ['-e', 'process.stdout.write(process.cwd())'] }))
    underHome.daemon.write(request('~/code', { args: ['-e', 'process.stdout.write(process.cwd())'] }))

    expect(await inHome.answer()).toEqual({ ok: true, cwd: home })
    expect(await underHome.answer()).toEqual({ ok: true, cwd: path.join(home, 'code') })
    await vi.waitFor(() => expect(underHome.stdout()).toBe(path.join(home, 'code')))
  })

  it('answers not_found for a command that is not on the PATH, and for a failed login-shell read', async () => {
    const missing = session()
    const noShell = session({
      environment: async () => {
        throw new Error('the login shell /bin/zsh exited with code 1')
      },
    })

    missing.daemon.write(request(directory(), { command: 'no-such-harness' }))
    noShell.daemon.write(request(directory()))

    expect(await missing.answer()).toEqual({
      ok: false,
      error: 'not_found',
      message: expect.stringContaining('no-such-harness'),
    })
    expect(await noShell.answer()).toEqual({
      ok: false,
      error: 'not_found',
      message: 'the login shell /bin/zsh exited with code 1',
    })
    // A refusal ends the stream, and no process ran, so nothing exited.
    await ended(missing.daemon)
    await ended(noShell.daemon)
    expect([...missing.exits, ...noShell.exits]).toEqual([])
  })

  it('answers bad_directory for a missing directory, a file and a relative path', async () => {
    const parent = directory()
    writeFileSync(path.join(parent, 'file'), '')

    for (const cwd of [path.join(parent, 'missing'), path.join(parent, 'file'), 'code/app']) {
      const { daemon, answer } = session()
      daemon.write(request(cwd))
      expect(await answer()).toEqual({ ok: false, error: 'bad_directory', message: expect.stringContaining(cwd) })
      await ended(daemon)
    }
  })

  it('answers spawn_failed for a malformed request line and for a line over 64 KiB', async () => {
    const cwd = directory()
    const lines = [
      'not json\n',
      `${JSON.stringify({ session_id: 'session-1', command: NODE })}\n`,
      request(cwd, { args: [1 as unknown as string] }),
      request(cwd, { env: { NAME: 2 as unknown as string } }),
      request(cwd, { command: '' }),
      request(cwd, { args: ['-e', COPY, 'x'.repeat(64 * 1024)] }),
    ]

    for (const line of lines) {
      const { daemon, answer, exits } = session()
      daemon.write(line)
      expect(await answer()).toEqual({ ok: false, error: 'spawn_failed', message: expect.any(String) })
      await ended(daemon)
      expect(exits).toEqual([])
    }
  })

  it('takes a request line of 64 KiB with its line feed', async () => {
    const cwd = directory()
    const short = request(cwd, { args: ['-e', COPY, ''] })
    const line = request(cwd, { args: ['-e', COPY, 'x'.repeat(64 * 1024 - Buffer.byteLength(short))] })
    expect(Buffer.byteLength(line)).toBe(64 * 1024)
    const { daemon, answer } = session()

    daemon.write(line)

    expect(await answer()).toEqual({ ok: true, cwd })
    daemon.end()
  })

  it('answers worktree_failed for a request with a worktree', async () => {
    const { daemon, answer } = session()

    daemon.write(request(directory(), { worktree: { repo: '/code/app', branch: 'pagis/fix', base: 'main' } }))

    expect(await answer()).toEqual({
      ok: false,
      error: 'worktree_failed',
      message: 'This Client App makes no worktree',
    })
    await ended(daemon)
  })

  it('answers spawn_failed when the process does not start', async () => {
    // An environment past the limit of the system: the exec fails.
    const { daemon, answer } = session({
      environment: async () => ({ ...SHELL_ENVIRONMENT, HUGE: 'x'.repeat(4 * 1024 * 1024) }),
    })

    daemon.write(request(directory()))

    expect(await answer()).toEqual({ ok: false, error: 'spawn_failed', message: expect.any(String) })
    await ended(daemon)
  })

  it('ends the stream and sends the exit code and the last 4 KiB of stderr when the process exits', async () => {
    const { daemon, exits, done, answer } = session()

    daemon.write(
      request(directory(), {
        args: ['-e', "process.stderr.write('a'.repeat(904) + 'b'.repeat(4096)); process.exitCode = 3"],
      }),
    )

    expect(await answer()).toMatchObject({ ok: true })
    await ended(daemon)
    await done
    expect(exits).toEqual([
      { type: 'session_exit', session_id: 'session-1', exit_code: 3, stderr_tail: 'b'.repeat(4096) },
    ])
  })

  it('sends no exit code when a signal ended the process', async () => {
    const { daemon, exits, done } = session()

    daemon.write(request(directory(), { args: ['-e', "process.kill(process.pid, 'SIGKILL')"] }))

    await done
    expect(exits).toEqual([{ type: 'session_exit', session_id: 'session-1', exit_code: null, stderr_tail: '' }])
  })

  it('kills the process and its child at once when the stream is reset', async () => {
    const { daemon, exits, done, answer, stdout } = session()
    daemon.write(request(directory(), { args: ['-e', WITH_CHILD] }))
    expect(await answer()).toMatchObject({ ok: true })
    await vi.waitFor(() => expect(stdout()).toMatch(/^\d+ \d+\n$/))
    const [leader, child] = stdout().trim().split(' ').map(Number)
    pids.push(leader, child)

    daemon.destroy()

    await done
    await vi.waitFor(() => expect([isAlive(leader), isAlive(child)]).toEqual([false, false]))
    expect(exits).toEqual([{ type: 'session_exit', session_id: 'session-1', exit_code: null, stderr_tail: '' }])
  })

  it('ends stdin on a FIN, and kills a process that ignores the end after the grace time', async () => {
    const { daemon, exits, done, answer, stdout } = session({ graceMs: 300 })
    daemon.write(
      request(directory(), {
        args: [
          '-e',
          [
            "process.stdin.on('end', () => process.stdout.write('end\\n')).resume()",
            "process.on('SIGTERM', () => process.stdout.write('term\\n'))",
            'setInterval(() => {}, 1000)',
          ].join(';'),
        ],
      }),
    )
    expect(await answer()).toMatchObject({ ok: true })

    daemon.end()
    await vi.waitFor(() => expect(stdout()).toBe('end\n'))
    await new Promise((resolve) => setTimeout(resolve, 150))
    expect(stdout()).toBe('end\n')
    // After the grace time it gets SIGTERM, which it ignores, and after a
    // second grace time SIGKILL.
    await vi.waitFor(() => expect(stdout()).toBe('end\nterm\n'))
    expect(exits).toEqual([])

    await done
    expect(exits).toEqual([{ type: 'session_exit', session_id: 'session-1', exit_code: null, stderr_tail: '' }])
  })
})

/** A session socket that a test drives. */
class FakeSessionSocket implements ByteSocket {
  readonly sent: Buffer[] = []
  closed = false
  private readonly messageListeners: Array<(bytes: Uint8Array) => void> = []
  private readonly closeListeners: Array<(code: number) => void> = []

  send(bytes: Uint8Array): void {
    this.sent.push(Buffer.from(bytes))
  }

  onMessage(listener: (bytes: Uint8Array) => void): void {
    this.messageListeners.push(listener)
  }

  onClose(listener: (code: number) => void): void {
    this.closeListeners.push(listener)
  }

  close(): void {
    this.closeWith(1005)
  }

  closeWith(code: number): void {
    if (this.closed) return
    this.closed = true
    for (const listener of this.closeListeners) listener(code)
  }

  deliver(bytes: Buffer): void {
    for (const listener of this.messageListeners) listener(bytes)
  }

  received(): Buffer {
    return Buffer.concat(this.sent)
  }
}

describe('the link that keeps the session socket open', () => {
  function linkOver(sockets: FakeSessionSocket[], sessionEnded = () => {}, onChange = () => {}) {
    const exits: SessionExitFrame[] = []
    const link = new SessionLink(
      async () => {
        const socket = new FakeSessionSocket()
        sockets.push(socket)
        return socket
      },
      {
        environment: async () => ({ ...SHELL_ENVIRONMENT }),
        send: (frame) => exits.push(frame),
        graceMs: 200,
      },
      1,
      sessionEnded,
      onChange,
    )
    return { link, exits }
  }

  /** The pids that the process of stream `streamId` printed. */
  async function pidsOf(socket: FakeSessionSocket, streamId: number): Promise<number[]> {
    await vi.waitFor(() => expect(streamBytes(socket.received(), streamId)).toMatch(/\n\d+ \d+\n$/))
    const printed = streamBytes(socket.received(), streamId).split('\n')[1].split(' ').map(Number)
    pids.push(...printed)
    return printed
  }

  it('counts the processes that run, and its stop resolves after every process exited', async () => {
    const sockets: FakeSessionSocket[] = []
    const onChange = vi.fn()
    const { link, exits } = linkOver(sockets, () => {}, onChange)
    const cwd = directory()
    link.start()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))

    sockets[0].deliver(
      Buffer.concat([
        openStream(1, request(cwd, { session_id: 'session-1', args: ['-e', WITH_CHILD] })),
        openStream(3, request(cwd, { session_id: 'session-2', args: ['-e', WITH_CHILD] })),
      ]),
    )
    const running = [...(await pidsOf(sockets[0], 1)), ...(await pidsOf(sockets[0], 3))]
    expect(link.running).toBe(2)
    expect(onChange).toHaveBeenCalled()

    await link.stop()

    // Each process exited. The kill of its group reaches its child too,
    // which the system reaps a moment later.
    const [first, firstChild, second, secondChild] = running
    expect([isAlive(first), isAlive(second)]).toEqual([false, false])
    await vi.waitFor(() => expect([isAlive(firstChild), isAlive(secondChild)]).toEqual([false, false]))
    expect(link.running).toBe(0)
    expect(sockets[0].closed).toBe(true)
    expect(exits.map((exit) => exit.session_id).sort()).toEqual(['session-1', 'session-2'])
  })

  it('kills the processes of a socket that closed, and opens the socket again', async () => {
    const sockets: FakeSessionSocket[] = []
    const { link } = linkOver(sockets)
    link.start()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))
    sockets[0].deliver(openStream(1, request(directory(), { args: ['-e', WITH_CHILD] })))
    const [leader, child] = await pidsOf(sockets[0], 1)

    sockets[0].closeWith(1006)

    await vi.waitFor(() => expect([isAlive(leader), isAlive(child)]).toEqual([false, false]))
    await vi.waitFor(() => expect(sockets).toHaveLength(2))
    await vi.waitFor(() => expect(link.running).toBe(0))
    await link.stop()
  })

  it('reports a close with 1008 as an ended Session, and no other close', async () => {
    const sockets: FakeSessionSocket[] = []
    const sessionEnded = vi.fn()
    const { link } = linkOver(sockets, sessionEnded)
    link.start()
    await vi.waitFor(() => expect(sockets).toHaveLength(1))

    sockets[0].closeWith(1006)
    await vi.waitFor(() => expect(sockets).toHaveLength(2))
    expect(sessionEnded).not.toHaveBeenCalled()

    sockets[1].closeWith(1008)
    await vi.waitFor(() => expect(sockets).toHaveLength(3))
    expect(sessionEnded).toHaveBeenCalledTimes(1)
    await link.stop()
  })

  it('keeps trying while the socket does not open, and opens nothing once it is stopped', async () => {
    let attempts = 0
    const link = new SessionLink(
      async () => {
        attempts += 1
        throw new Error('the Host socket has not registered')
      },
      { send: () => {} },
      1,
    )

    link.start()
    await vi.waitFor(() => expect(attempts).toBeGreaterThan(2))
    await link.stop()
    const settled = attempts
    await new Promise((resolve) => setTimeout(resolve, 20))

    expect(attempts).toBe(settled)
  })
})

/** The interop harness that a test of the daemon runs: plain node, with
 *  no package, from the root of the repository. */
describe('the interop harness of the daemon', () => {
  const ROOT = path.join(__dirname, '..', '..')
  const resources: Array<{ close: () => Promise<void> }> = []
  afterEach(async () => {
    for (const resource of resources.splice(0)) await resource.close()
  })

  it('registers a Host with a harness, serves its session socket, and sends the exit on the Host socket', async () => {
    const server = await daemonServer()
    resources.push(server)
    const child = spawn(
      process.execPath,
      ['--disable-warning=MODULE_TYPELESS_PACKAGE_JSON', 'desktop/test/session-peer.ts', server.origin, 'secret'],
      { cwd: ROOT, stdio: ['pipe', 'pipe', 'pipe'] },
    )
    const output = { stdout: '', stderr: '' }
    child.stdout.on('data', (chunk: Buffer) => (output.stdout += chunk.toString()))
    child.stderr.on('data', (chunk: Buffer) => (output.stderr += chunk.toString()))
    const exited = new Promise<number | null>((resolve) => child.once('exit', (code) => resolve(code)))
    resources.push({
      close: async () => {
        child.kill()
        await exited
      },
    })
    const texts = (index: number) =>
      server.ends[index].frames
        .filter((frame) => frame.opcode === TEXT)
        .map((frame) => JSON.parse(frame.payload.toString()) as Record<string, unknown>)

    await vi.waitFor(() => expect(texts(0)).toHaveLength(2), { timeout: 10_000 })
    const [hostEnd] = server.ends
    expect(hostEnd.url).toBe('/api/v1/ws')
    expect(hostEnd.headers.cookie).toBe('pagis_session=secret')
    expect(texts(0)[1]).toMatchObject({ type: 'register_host', capabilities: ['shell', 'harness:claude'] })
    hostEnd.text(JSON.stringify({ type: 'ready' }))
    hostEnd.text(
      JSON.stringify({ type: 'host.registered', payload: { host_id: 'host-1', capabilities: ['shell', 'harness:claude'] } }),
    )
    await vi.waitFor(() => expect(output.stdout).toBe('ready host-1\n'))
    await vi.waitFor(() => expect(server.ends).toHaveLength(2))
    const sessionEnd = server.ends[1]
    expect(sessionEnd.url).toBe('/api/v1/hosts/host-1/sessions')
    expect(sessionEnd.headers.cookie).toBe('pagis_session=secret')

    sessionEnd.binary(openStream(1, `${request(directory(), { command: 'node' }).trimEnd()}\nhello\n`))

    await vi.waitFor(() => expect(streamBytes(sessionEnd.received(), 1)).toMatch(/^\{"ok":true,"cwd":".*"\}\nhello\n$/))
    // The daemon ends its side: the process exits at the end of its input.
    sessionEnd.binary(yamuxFrame(DATA, FIN, 1, 0))
    await vi.waitFor(() =>
      expect(texts(0)).toContainEqual({ type: 'session_exit', session_id: 'session-1', exit_code: 0, stderr_tail: '' }),
    )

    child.stdin.end()
    expect(await exited).toBe(0)
    expect(output.stderr).toBe('')
  })
})
