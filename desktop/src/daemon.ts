import { type ChildProcess, spawn } from 'node:child_process'
import { EventEmitter } from 'node:events'

import {
  daemonUrl,
  readAdministrationPort,
  readClientCredential,
  readPort,
  writePort,
} from './dataDirectory'
import { sleep } from './health'
import { PidFile } from './pidFile'
import { nextFreePort } from './ports'
import { parseTakenAdministrationPort, parseTakenPort, portHolder } from './takenPort'
import { probeRuntimeIdentity, waitForRuntimeIdentity, type RuntimeIdentity } from './runtimeIdentity'

/**
 * The exit code the daemon uses for "start me again" (ADR-0025).
 * `POST /api/v1/system/restart` answers with it as well.
 */
export const RESTART_EXIT_CODE = 75
/** Crashes the shell rides out before it gives up. */
export const MAX_RESTARTS = 3
/** The pause between one crash and the next start. */
export const RESTART_PAUSE_MS = 1000
/** How long a fresh daemon has to answer the health endpoint. */
export const HEALTH_TIMEOUT_MS = 30000
/** Log lines kept for the failure page. */
const LOG_LINES = 200

export type DaemonState =
  | { kind: 'starting' }
  /** `owned` is false for a daemon the shell attached to. */
  | { kind: 'running'; url: string; port: number; version: string; owned: boolean }
  | { kind: 'taken-port'; port: number; holder: string; suggested: number }
  | { kind: 'failed'; reason: string; log: string }

export interface SupervisorOptions {
  /** The data directory: config file, Client Credential, database and logs. */
  home: string
  /** The daemon binary, from `Contents/Resources` when packaged. */
  binaryPath: string
  /** The daemon version this app ships. A daemon that reports it may be attached to. */
  version: string
  /** Immutable Computer image digest compiled into the locked server. */
  computerImage: string
  pidFile: PidFile
  restartPauseMs?: number
  healthTimeoutMs?: number
  /** Persist the release boundary immediately before this supervisor spawns. */
  beforeSpawn?: () => void
}

/**
 * The shell owns the daemon (ADR-0025). This class holds the whole
 * lifecycle: attach to a healthy daemon of our version, replace one of
 * another version, spawn our own, restart it when it crashes or asks
 * for a restart, report a taken port, and stop it on quit.
 *
 * It emits `state` on every change; the shell's windows render it.
 */
export class DaemonSupervisor extends EventEmitter {
  private child: ChildProcess | null = null
  private restarts = 0
  private stopping = false
  private diagnostics = new DiagnosticLog(LOG_LINES)
  private current: DaemonState = { kind: 'starting' }
  private queue: Promise<void> = Promise.resolve()
  private startJob: Promise<void> | null = null
  private intent = 0

  constructor(private readonly options: SupervisorOptions) {
    super()
  }

  get state(): DaemonState {
    return this.current
  }

  /** The running daemon's process id, for tests and for the log page. */
  get pid(): number | null {
    return this.child?.pid ?? null
  }

  /**
   * Take the daemon to a running state. Safe to call again: Retry on
   * the status page calls it.
   */
  async start(): Promise<void> {
    if (this.startJob) return this.startJob
    const intent = ++this.intent
    this.stopping = false
    const job = this.enqueue(() => this.startOnce(intent))
    this.startJob = job
    const clear = (): void => { if (this.startJob === job) this.startJob = null }
    job.then(clear, clear)
    return job
  }

  private async startOnce(intent: number): Promise<void> {
    if (intent !== this.intent || this.stopping) return
    if (this.child && this.current.kind === 'running' && this.current.owned) return
    this.setState({ kind: 'starting' })
    // A PID from an earlier client is evidence only. It does not grant authority
    // to signal a process because the PID may now name another process.
    this.options.pidFile.clear()

    const port = readPort(this.options.home)
    const credential = readClientCredential(this.options.home)
    const found = credential
      ? await probeRuntimeIdentity(daemonUrl(port), port, credential, null, null)
      : null
    if (intent !== this.intent || this.stopping) return
    if (found) {
      if (
        found.release === this.options.version &&
        found.computerImage === this.options.computerImage
      ) {
        this.attach(port, found)
        return
      }
      this.setState({
        kind: 'failed',
        reason:
          `Pagis ${found.release} is running on port ${port}. This app ships ` +
          `Pagis ${this.options.version}. Quit the other Pagis server, then select Retry.`,
        log: this.logTail(),
      })
      return
    }

    this.restarts = 0
    await this.spawnDaemon(port, intent)
  }

  /**
   * Take the port the taken-port page proposes: write it into the
   * config file, the one key the shell writes, and start again.
   */
  async usePort(port: number): Promise<void> {
    const intent = ++this.intent
    this.stopping = false
    const child = this.takeChild()
    await this.enqueue(async () => {
      if (child) await stopOwnedChild(child)
      if (intent !== this.intent) return
      writePort(this.options.home, port)
      this.restarts = 0
      await this.startOnce(intent)
    })
  }

  /**
   * Stop the daemon this shell spawned. Quit calls it. A daemon the
   * shell only attached to belongs to whoever started it, so it stays.
   */
  async stop(): Promise<void> {
    this.stopping = true
    this.intent += 1
    const child = this.takeChild()
    await this.enqueue(async () => { if (child) await stopOwnedChild(child) })
  }

  /** The tail of the daemon's output, for the failure page. */
  logTail(): string {
    return this.diagnostics.tail()
  }

  private attach(port: number, identity: RuntimeIdentity): void {
    this.setState({
      kind: 'running',
      url: daemonUrl(port),
      port,
      version: identity.release,
      owned: false,
    })
  }

  private async spawnDaemon(port: number, intent: number): Promise<void> {
    if (intent !== this.intent || this.stopping) return
    this.options.beforeSpawn?.()
    this.setState({ kind: 'starting' })
    // The log holds this run alone: the failure page shows why the
    // daemon stopped, and a taken port is read from what it printed.
    this.diagnostics = new DiagnosticLog(LOG_LINES)
    // `--local`: this is a local installation, so the daemon holds a
    // Client Credential for this client, whatever its Public Origin.
    // No `--port`: that flag is a run-only override, and the port the
    // daemon must bind is the one in the config file (ADR-0025).
    // `PAGIS_SUPERVISED`: this shell starts the daemon again when it
    // exits for a restart, so the Administration Interface says so.
    const child = spawn(this.options.binaryPath, ['--no-open', '--local'], {
      env: { ...process.env, PAGIS_HOME: this.options.home, PAGIS_SUPERVISED: '1' },
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    this.child = child
    if (child.pid !== undefined) {
      this.options.pidFile.write(child.pid)
    }
    child.stdout?.on('data', (chunk: Buffer) => this.collect(chunk, 'stdout'))
    child.stderr?.on('data', (chunk: Buffer) => this.collect(chunk, 'stderr'))
    child.on('error', (error) => {
      this.collect(Buffer.from(`${error.message}\n`), 'stderr')
    })
    // `close`, not `exit`: the last of the daemon's output must be in
    // the log before the taken-port message is read from it. An exit
    // while the start waits for the identity belongs to this start, so
    // the start ends on the state that the exit gives: a taken port, a
    // restart or a failure. A later exit is a crash of a running server.
    let starting = true
    let exitedWhileStarting: number | null = null
    child.on('close', (code) => {
      this.diagnostics.finish()
      if (this.child !== child) return
      this.child = null
      this.options.pidFile.clear()
      if (starting) {
        exitedWhileStarting = code ?? 1
        return
      }
      void this.enqueue(() => this.handleExit(code ?? 1, intent))
    })

    const identity = await waitForRuntimeIdentity(
      daemonUrl(port),
      port,
      () => readClientCredential(this.options.home),
      this.options.version,
      this.options.computerImage,
      {
      timeoutMs: this.options.healthTimeoutMs ?? HEALTH_TIMEOUT_MS,
      cancelled: () => this.child !== child || intent !== this.intent || this.stopping,
      },
    )
    starting = false
    if (exitedWhileStarting !== null) {
      await this.handleExit(exitedWhileStarting, intent)
      return
    }
    // Quit or a port change took the child while the start waited.
    if (this.child !== child) return
    if (!identity) {
      const timedOut = this.takeChild(child)
      if (timedOut) await stopOwnedChild(timedOut)
      this.setState({
        kind: 'failed',
        reason: `The Pagis server did not prove its identity on port ${port}. Select Retry to start it again.`,
        log: this.logTail(),
      })
      return
    }
    if (child.exitCode !== null || child.signalCode !== null) {
      this.setState({
        kind: 'failed',
        reason: 'The verified Pagis server exited before setup completed.',
        log: this.logTail(),
      })
      return
    }
    this.setState({
      kind: 'running',
      url: daemonUrl(port),
      port,
      version: identity.release,
      owned: true,
    })
  }

  private async handleExit(code: number, intent: number): Promise<void> {
    try {
      await this.onExit(code, intent)
    } catch (error) {
      this.setState({
        kind: 'failed',
        reason: `The daemon could not be started again: ${redact(String(error))}`,
        log: this.logTail(),
      })
    }
  }

  private async onExit(code: number, intent: number): Promise<void> {
    if (intent !== this.intent || this.stopping) return

    if (code === RESTART_EXIT_CODE) {
      // The user asked for a restart, so this is not a crash: the
      // restart budget stays as it is, and the port is read again
      // because "Restart now" may have changed it.
      await this.spawnDaemon(readPort(this.options.home), intent)
      return
    }

    const taken = parseTakenPort(this.logTail())
    if (taken !== null) {
      this.setState({
        kind: 'taken-port',
        port: taken,
        holder: (await portHolder(taken)) ?? 'another process',
        suggested: await nextFreePort(taken + 1, [readAdministrationPort(this.options.home)]),
      })
      return
    }

    // A taken Administration Port does not go away on a restart, and
    // only the config file moves it.
    const administration = parseTakenAdministrationPort(this.logTail())
    if (administration !== null) {
      const holder = (await portHolder(administration)) ?? 'another process'
      this.setState({
        kind: 'failed',
        reason:
          `${holder} uses port ${administration}, the Administration Port of Pagis, so Pagis ` +
          'cannot start. Stop that process, or name another port in [administration] port ' +
          'of config.toml. Then start Pagis again.',
        log: this.logTail(),
      })
      return
    }

    this.restarts += 1
    if (this.restarts > MAX_RESTARTS) {
      this.setState({
        kind: 'failed',
        reason: `The daemon stopped ${this.restarts} times with code ${code}.`,
        log: this.logTail(),
      })
      return
    }
    await sleep(this.options.restartPauseMs ?? RESTART_PAUSE_MS)
    if (this.stopping || intent !== this.intent) return
    await this.spawnDaemon(readPort(this.options.home), intent)
  }

  private collect(chunk: Buffer, stream: 'stdout' | 'stderr'): void {
    this.diagnostics.append(chunk, stream)
  }

  private setState(state: DaemonState): void {
    this.current = state
    this.emit('state', state)
  }

  private enqueue(operation: () => Promise<void>): Promise<void> {
    const result = this.queue.then(operation, operation)
    this.queue = result.catch(() => undefined)
    return result
  }

  private takeChild(expected?: ChildProcess): ChildProcess | null {
    const child = this.child
    if (!child || (expected && child !== expected)) return null
    this.child = null
    this.options.pidFile.clear()
    return child
  }
}

async function stopOwnedChild(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return
  child.kill('SIGINT')
  if (await closesWithin(child, 5000)) return
  child.kill('SIGKILL')
  await closesWithin(child, 1000)
}

function closesWithin(child: ChildProcess, timeoutMs: number): Promise<boolean> {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve(true)
  return new Promise((resolve) => {
    const timer = setTimeout(() => { child.off('close', closed); resolve(false) }, timeoutMs)
    function closed(): void { clearTimeout(timer); resolve(true) }
    child.once('close', closed)
  })
}

export function redact(value: string): string {
  return value
    .replace(/(\/api\/v1\/sessions\/link\/)[^\s"'/?&]+/gi, '$1[redacted]')
    .replace(/(pagis_session=)[^;\s"']+/gi, '$1[redacted]')
    .replace(/("?(?:credential|token|api[_-]?key|secret)"?\s*[:=]\s*["']?)[^\s,"'}]+/gi, '$1[redacted]')
}

export class DiagnosticLog {
  private lines: string[] = []
  private readonly streams = new Map<string, { fragment: string; discarding: boolean }>()

  constructor(private readonly maxLines: number) {}

  append(chunk: Buffer, stream = 'stdout'): void {
    const state = this.streams.get(stream) ?? { fragment: '', discarding: false }
    let input = chunk.toString('utf8')
    if (state.discarding) {
      const newline = input.indexOf('\n')
      if (newline === -1) { this.streams.set(stream, state); return }
      state.discarding = false
      input = input.slice(newline + 1)
    }
    let combined = state.fragment + input
    state.fragment = ''
    while (true) {
      const newline = combined.indexOf('\n')
      if (newline === -1) break
      const line = combined.slice(0, newline)
      if (line.length > 1024 * 1024) this.push('[daemon output line exceeded the diagnostic limit]')
      else this.push(line)
      combined = combined.slice(newline + 1)
    }
    if (combined.length > 1024 * 1024) {
      this.push('[daemon output line exceeded the diagnostic limit]')
      state.discarding = true
    } else {
      state.fragment = combined
    }
    this.streams.set(stream, state)
  }

  finish(): void {
    for (const state of this.streams.values()) {
      if (!state.discarding && state.fragment !== '') this.push(state.fragment)
    }
    this.streams.clear()
  }

  tail(): string { return this.lines.join('\n') }

  private push(line: string): void {
    if (line.trim() === '') return
    this.lines.push(redact(line))
    if (this.lines.length > this.maxLines) this.lines = this.lines.slice(-this.maxLines)
  }
}
