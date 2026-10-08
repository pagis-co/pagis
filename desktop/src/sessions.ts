// The Coding Sessions of this Host (ADR-0033).
//
// The daemon opens one yamux stream on the session socket, a further
// WebSocket of this client, for each Coding Session. For each stream the
// client reads the open request, starts the Coding Harness that it names,
// answers, and copies the bytes of the harness's stdin and stdout. The
// client stays a pipe: it reads no ACP message.
//
// The stream protocol, which the daemon implements the same way:
//
// 1. The daemon writes the open request: one JSON line of at most 64 KiB
//    with its line feed, `{"session_id", "command", "args", "cwd", "env",
//    "worktree"}`.
// 2. The client answers with one JSON line before any other byte:
//    `{"ok": true, "cwd": "<the directory the process runs in>"}`, or
//    `{"ok": false, "error": "<code>", "message": "..."}`. The codes are
//    `not_found`, `bad_directory`, `worktree_failed` and `spawn_failed`.
//    After a refusal, the client ends the stream.
// 3. After `ok`, the stream carries the raw stdin and stdout of the
//    process. When the process exits, the client ends the stream and sends
//    a `session_exit` frame on its Host socket.
//
// The process ends with its stream. A FIN of the daemon ends its stdin,
// and the client kills a process that still runs a grace time later. A
// reset of the stream, or the end of the session socket, kills it at once.
// A process that outlives its stream answers nobody.
//
// This module uses only erasable TypeScript syntax and imports only Node
// built-ins, `./byteSocket`, `./loginShell`, `./streamLine` and `./yamux`,
// so plain `node` loads it in the interop test of the daemon
// (`test/session-peer.ts`).

import { spawn } from 'node:child_process'
import type { ChildProcessByStdio } from 'node:child_process'
import { stat } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { pipeline } from 'node:stream'
import type { Duplex, Readable, Writable } from 'node:stream'

import type { ByteSocket } from './byteSocket'
import { openByteSocket } from './byteSocket'
import { findOnPath, loginShellEnvironment } from './loginShell'
import { readLine } from './streamLine'
import { YamuxSession } from './yamux'

/** The longest open request, with its line feed. */
const REQUEST_LIMIT = 64 * 1024

const REQUEST_TIMEOUT_MS = 10_000

/** How long a process has after the end of its input, and after SIGTERM. */
const GRACE_MS = 5_000

/** The most bytes of stderr that the client keeps of a process. */
const STDERR_TAIL_LIMIT = 4 * 1024

/** The close code of a session socket whose Session ended, the same code
 *  as that of the Host socket: 1008, policy violation. */
const SESSION_ENDED = 1008

/** Why the client did not start the process. */
export type OpenFailure = 'not_found' | 'bad_directory' | 'worktree_failed' | 'spawn_failed'

/** The daemon's first line on a stream: what to start, and where. */
export interface OpenRequest {
  session_id: string
  command: string
  args: string[]
  cwd: string
  env: Record<string, string>
  worktree: { repo: string; branch: string; base: string } | null
}

/** The Host socket frame that tells how the process of a session ended.
 *  `exit_code` is `null` when a signal ended the process. */
export interface SessionExitFrame {
  type: 'session_exit'
  session_id: string
  exit_code: number | null
  stderr_tail: string
}

export interface SessionDeps {
  /** Send one frame on the Host socket. */
  send: (frame: SessionExitFrame) => void
  /** The environment of the person's login shell. The default reads it. */
  environment?: () => Promise<Record<string, string>>
  /** The home directory of the OS user. */
  home?: string
  /** How long a process has after the end of its input, and after
   *  SIGTERM. */
  graceMs?: number
  /** Hears that the process started. */
  onSpawn?: () => void
  /** Hears that the process that started exited. */
  onExit?: () => void
}

/** A request that the client refuses, with the code of its answer. */
class Refusal extends Error {
  readonly code: OpenFailure

  constructor(code: OpenFailure, message: string) {
    super(message)
    this.code = code
  }
}

type Harness = ChildProcessByStdio<Writable, Readable, Readable>

/**
 * Serve one session stream: read the open request, start the process,
 * answer, and pipe the stdio of the process. It resolves when the stream
 * is done: after a refusal, or after the process exited.
 */
export async function startSession(stream: Duplex, deps: SessionDeps): Promise<void> {
  // A reset of the stream or of its session is how a session ends early.
  // The 'close' that follows it kills the process.
  stream.on('error', () => {})
  let request: OpenRequest
  let file: string
  let cwd: string
  let env: Record<string, string>
  try {
    request = parseRequest(await readLine(stream, REQUEST_LIMIT, REQUEST_TIMEOUT_MS))
    if (request.worktree !== null) {
      throw new Refusal('worktree_failed', 'This Client App makes no worktree')
    }
    cwd = await resolveDirectory(request.cwd, deps.home ?? os.homedir())
    let shell: Record<string, string>
    try {
      shell = await (deps.environment ?? loginShellEnvironment)()
    } catch (error) {
      throw new Refusal('not_found', messageOf(error))
    }
    const found = await findOnPath(request.command, shell.PATH ?? '')
    if (found === null) {
      throw new Refusal('not_found', `${request.command} is not on the PATH of the login shell`)
    }
    file = found
    env = { ...shell, ...request.env }
  } catch (error) {
    refuse(stream, error instanceof Refusal ? error.code : 'spawn_failed', messageOf(error))
    return
  }
  if (stream.destroyed) return
  let child: Harness
  try {
    child = spawn(file, request.args, {
      cwd,
      env,
      stdio: ['pipe', 'pipe', 'pipe'],
      // The process leads its own process group, so a kill reaches the
      // processes that it started, such as those of `npx`.
      detached: true,
    })
  } catch (error) {
    // Node.js throws some failures of the exec, such as E2BIG, at once.
    refuse(stream, 'spawn_failed', messageOf(error))
    return
  }
  const started = await new Promise<boolean>((resolve) => {
    child.once('spawn', () => resolve(true))
    child.once('error', (error) => {
      refuse(stream, 'spawn_failed', messageOf(error))
      resolve(false)
    })
  })
  if (!started) return
  // An error after the start, such as a failed kill, changes nothing that
  // the 'close' of the process does not tell.
  child.on('error', () => {})
  deps.onSpawn?.()
  await carry(stream, child, cwd, request.session_id, deps)
  deps.onExit?.()
}

/** Answer `ok`, pipe the stdio of the process, and end it with its
 *  stream. It resolves when the process exited and its exit went out. */
function carry(stream: Duplex, child: Harness, cwd: string, sessionId: string, deps: SessionDeps): Promise<void> {
  const graceMs = deps.graceMs ?? GRACE_MS
  let exited = false
  let killTimer: ReturnType<typeof setTimeout> | null = null
  const kill = (): void => {
    if (exited || killTimer !== null) return
    signalGroup(child.pid, 'SIGTERM')
    killTimer = setTimeout(() => signalGroup(child.pid, 'SIGKILL'), graceMs)
  }

  stream.write(`${JSON.stringify({ ok: true, cwd })}\n`)
  return new Promise((resolve) => {
    let tail = Buffer.alloc(0)
    child.stderr.on('data', (chunk: Buffer) => {
      tail = Buffer.concat([tail, chunk])
      if (tail.length > STDERR_TAIL_LIMIT) tail = tail.subarray(tail.length - STDERR_TAIL_LIMIT)
    })
    // Each pipeline destroys both sides on an error of either. A reset of
    // the stream ends the session, and the 'close' of the stream kills
    // the process.
    pipeline(stream, child.stdin, () => {})
    pipeline(child.stdout, stream, () => {})
    // A FIN of the daemon ended stdin. An ACP agent exits at the end of
    // its input, and one that does not is killed.
    let graceTimer: ReturnType<typeof setTimeout> | null = null
    stream.once('end', () => {
      graceTimer = setTimeout(kill, graceMs)
    })
    // The stream can close while the process starts, before this
    // listener.
    if (stream.destroyed) kill()
    else stream.once('close', kill)
    child.once('close', (code: number | null) => {
      exited = true
      if (graceTimer !== null) clearTimeout(graceTimer)
      if (killTimer !== null) clearTimeout(killTimer)
      deps.send({
        type: 'session_exit',
        session_id: sessionId,
        exit_code: code,
        stderr_tail: tail.toString('utf8'),
      })
      resolve()
    })
  })
}

/** The open request of one line, with each field checked. */
function parseRequest(line: string): OpenRequest {
  let parsed: unknown
  try {
    parsed = JSON.parse(line)
  } catch {
    throw new Error('the open request is not JSON')
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    throw new Error('the open request is not a JSON object')
  }
  const request = parsed as Record<string, unknown>
  const { session_id: sessionId, command, args, cwd, env, worktree } = request
  if (typeof sessionId !== 'string' || sessionId === '') throw new Error('the open request names no `session_id`')
  if (typeof command !== 'string' || command === '') throw new Error('the open request names no `command`')
  if (!isStringArray(args)) throw new Error('the `args` of the open request are not a list of strings')
  if (typeof cwd !== 'string') throw new Error('the open request names no `cwd`')
  if (!isStringRecord(env)) throw new Error('the `env` of the open request is not an object of strings')
  if (worktree !== null && worktree !== undefined && !isWorktree(worktree)) {
    throw new Error('the `worktree` of the open request is not a `repo`, a `branch` and a `base`')
  }
  return {
    session_id: sessionId,
    command,
    args,
    cwd,
    env,
    worktree: (worktree as OpenRequest['worktree'] | undefined) ?? null,
  }
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === 'string')
}

function isStringRecord(value: unknown): value is Record<string, string> {
  return (
    typeof value === 'object' &&
    value !== null &&
    !Array.isArray(value) &&
    Object.values(value).every((item) => typeof item === 'string')
  )
}

function isWorktree(value: unknown): boolean {
  if (typeof value !== 'object' || value === null) return false
  const { repo, branch, base } = value as Record<string, unknown>
  return typeof repo === 'string' && typeof branch === 'string' && typeof base === 'string'
}

/**
 * The directory of the process. `~`, and a path that starts with `~/`,
 * resolve against the home directory of the OS user, because the daemon
 * does not know it. Any other path must be absolute.
 */
async function resolveDirectory(cwd: string, home: string): Promise<string> {
  const resolved = cwd === '~' ? home : cwd.startsWith('~/') ? path.join(home, cwd.slice(2)) : cwd
  if (!path.isAbsolute(resolved)) {
    throw new Refusal('bad_directory', `the directory ${cwd} is not an absolute path`)
  }
  let isDirectory: boolean
  try {
    isDirectory = (await stat(resolved)).isDirectory()
  } catch (error) {
    throw new Refusal('bad_directory', `the directory ${cwd} cannot be read: ${messageOf(error)}`)
  }
  if (!isDirectory) throw new Refusal('bad_directory', `${cwd} is not a directory`)
  return resolved
}

/** Answer with a refusal and end the stream. */
function refuse(stream: Duplex, code: OpenFailure, message: string): void {
  if (stream.destroyed || stream.writableEnded) return
  stream.end(`${JSON.stringify({ ok: false, error: code, message })}\n`)
  // Nothing reads the rest of the stream, so it can close once the daemon
  // closes its side.
  stream.resume()
}

/** Send `signal` to the process group that the process leads. */
function signalGroup(pid: number | undefined, signal: NodeJS.Signals): void {
  if (pid === undefined) return
  try {
    process.kill(-pid, signal)
  } catch (error) {
    // The group is gone when its processes exited at the same time. On
    // macOS, a group of processes that exited and that Node.js has not
    // reaped yet answers EPERM.
    const code = (error as NodeJS.ErrnoException).code
    if (code !== 'ESRCH' && code !== 'EPERM') throw error
  }
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** The session socket of the Host with the id `hostId`, a [`ByteSocket`]
 *  on the session path. */
export function openSessionSocket(url: string, sessionSecret: string, hostId: string): Promise<ByteSocket> {
  return openByteSocket(url, sessionSecret, `/api/v1/hosts/${encodeURIComponent(hostId)}/sessions`)
}

/**
 * Keep the session socket open for as long as the client runs, and start
 * a Coding Harness for each stream that the daemon opens on it.
 *
 * A socket that closes takes its streams and their processes with it, and
 * the link opens another one. A socket that closes with code 1008 lost
 * its Session: the link tells `sessionEnded`, and the next `open` must not
 * use that Session.
 */
export class SessionLink {
  private readonly open: () => Promise<ByteSocket>
  private readonly deps: SessionDeps
  private readonly retryMs: number
  private readonly sessionEnded: () => void
  private readonly onChange: () => void
  private socket: ByteSocket | null = null
  private session: YamuxSession | null = null
  private stopped = false
  private timer: ReturnType<typeof setTimeout> | null = null
  private count = 0
  /** Each stream that is not done: its request, its process or its exit. */
  private readonly streams = new Set<Promise<void>>()

  constructor(
    open: () => Promise<ByteSocket>,
    deps: SessionDeps,
    retryMs = 3_000,
    sessionEnded: () => void = () => {},
    onChange: () => void = () => {},
  ) {
    this.open = open
    this.deps = deps
    this.retryMs = retryMs
    this.sessionEnded = sessionEnded
    this.onChange = onChange
  }

  /** The processes that run now. */
  get running(): number {
    return this.count
  }

  /** Open the socket, and keep doing so until [`stop`]. */
  start(): void {
    this.stopped = false
    void this.connect()
  }

  /** Close the socket and kill every process. It resolves when each
   *  process exited. */
  async stop(): Promise<void> {
    this.stopped = true
    if (this.timer !== null) clearTimeout(this.timer)
    this.timer = null
    this.session?.close()
    this.session = null
    this.socket?.close()
    this.socket = null
    await Promise.all(this.streams)
  }

  private serve(stream: Duplex): void {
    const done = startSession(stream, {
      ...this.deps,
      onSpawn: () => {
        this.count += 1
        this.onChange()
      },
      onExit: () => {
        this.count -= 1
        this.onChange()
      },
    })
    this.streams.add(done)
    void done.finally(() => this.streams.delete(done))
  }

  private async connect(): Promise<void> {
    if (this.stopped) return
    let socket: ByteSocket
    try {
      socket = await this.open()
    } catch {
      this.retry()
      return
    }
    if (this.stopped) {
      socket.close()
      return
    }
    const session = new YamuxSession({
      send: (bytes) => socket.send(bytes),
      onStream: (stream) => this.serve(stream),
      // The daemon ended the session or broke the protocol. A new socket
      // starts a new session.
      onEnd: () => socket.close(),
    })
    this.socket = socket
    this.session = session
    socket.onMessage((bytes) => session.receive(bytes))
    socket.onClose((code) => {
      session.abort()
      if (this.socket !== socket) return
      this.socket = null
      this.session = null
      if (code === SESSION_ENDED) this.sessionEnded()
      this.retry()
    })
  }

  private retry(): void {
    if (this.stopped || this.timer !== null) return
    this.timer = setTimeout(() => {
      this.timer = null
      void this.connect()
    }, this.retryMs)
  }
}
