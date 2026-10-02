import { type ChildProcess, spawn } from 'node:child_process'
import { EventEmitter } from 'node:events'
import * as fs from 'node:fs'
import * as net from 'node:net'
import * as os from 'node:os'
import * as path from 'node:path'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { DaemonSupervisor, DiagnosticLog, STOP_GRACE_MS, redact, stopOwnedChild, type DaemonState } from './daemon'
import { configPath, readAdministrationPort, readPort } from './dataDirectory'
import { sleep, waitForHealth } from './health'
import { PidFile } from './pidFile'
import { isFree, nextFreePort } from './ports'
import { isAlive, stopProcess } from './processes'
import { RuntimeState } from './runtimeState'
import { portHolder } from './takenPort'

const FAKE_DAEMON = path.join(__dirname, '..', 'test', 'fake-daemon.mjs')
const OUR_VERSION = '0.1.0'
const OUR_IMAGE = `ghcr.io/pagis-co/pagis-computer@sha256:${'b'.repeat(64)}`

const cleanups: (() => void | Promise<void>)[] = []

afterEach(async () => {
  while (cleanups.length > 0) {
    await cleanups.pop()!()
  }
})

/** A data directory with a free port already written into its config. */
async function newHome(): Promise<{ home: string; port: number }> {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-shell-'))
  cleanups.push(() => fs.rmSync(home, { recursive: true, force: true }))
  const port = await nextFreePort(14400 + Math.floor(Math.random() * 400))
  fs.writeFileSync(configPath(home), `port = ${port}\nlog_level = "info"\n`)
  return { home, port }
}

/**
 * A data directory whose config names the Administration Port right
 * after the product port, as the defaults 4400 and 4401 do. Both ports
 * are free, so the next free port after the product port is the
 * Administration Port.
 */
async function newHomeWithAdministrationPort(): Promise<{ home: string; port: number; administration: number }> {
  const { home } = await newHome()
  let port = await nextFreePort(14800 + Math.floor(Math.random() * 400))
  while (!(await isFree(port + 1))) port = await nextFreePort(port + 2)
  const administration = port + 1
  fs.writeFileSync(configPath(home), `port = ${port}\n\n[administration]\nport = ${administration}\n`)
  return { home, port, administration }
}

/** A process that listens on `port` and answers nothing. */
async function holdPort(port: number): Promise<void> {
  const server = net.createServer()
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(port, '127.0.0.1', () => resolve())
  })
  cleanups.push(() => new Promise<void>((resolve) => server.close(() => resolve())))
}

function newSupervisor(
  home: string,
  overrides: Partial<{
    version: string
    beforeSpawn: () => void
  }> = {},
): DaemonSupervisor {
  const supervisor = new DaemonSupervisor({
    home,
    binaryPath: FAKE_DAEMON,
    version: overrides.version ?? OUR_VERSION,
    computerImage: OUR_IMAGE,
    pidFile: PidFile.inside(home),
    restartPauseMs: 20,
    healthTimeoutMs: 5000,
    beforeSpawn: overrides.beforeSpawn,
  })
  cleanups.push(() => supervisor.stop())
  return supervisor
}

/** Wait for a state the test asks about, or fail with what came instead. */
function waitForState(
  supervisor: DaemonSupervisor,
  matches: (state: DaemonState) => boolean,
  timeoutMs = 10000,
): Promise<DaemonState> {
  if (matches(supervisor.state)) {
    return Promise.resolve(supervisor.state)
  }
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      supervisor.off('state', listener)
      reject(new Error(`the state stayed ${JSON.stringify(supervisor.state)}`))
    }, timeoutMs)
    function listener(state: DaemonState): void {
      if (matches(state)) {
        clearTimeout(timer)
        supervisor.off('state', listener)
        resolve(state)
      }
    }
    supervisor.on('state', listener)
  })
}

/** A fake daemon started outside the shell: a daemon it did not spawn.
 *  It is a local installation, as a daemon an earlier client started is. */
async function startRival(
  home: string,
  port: number,
  environment: NodeJS.ProcessEnv,
): Promise<ChildProcess> {
  const rival = spawn(FAKE_DAEMON, ['--local'], {
    env: { ...process.env, PAGIS_HOME: home, ...environment },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  cleanups.push(async () => {
    if (rival.pid !== undefined) {
      await stopProcess(rival.pid, 2000)
    }
  })
  if (environment.FAKE_MODE !== 'hold') {
    await waitForHealth(`http://127.0.0.1:${port}/`, { timeoutMs: 5000 })
  } else {
    // The holder answers no health endpoint, so wait for the socket.
    for (let tries = 0; tries < 100; tries += 1) {
      const answered = await fetch(`http://127.0.0.1:${port}/`).catch(() => null)
      if (answered) {
        break
      }
      await sleep(50)
    }
  }
  return rival
}

describe('the daemon supervisor', () => {
  it('redacts a sign-in link split across process output chunks', () => {
    const log = new DiagnosticLog(200)
    log.append(Buffer.from('open http://127.0.0.1:4400/api/v1/sessions/li'))
    log.append(Buffer.from('nk/do-not-print\n'))

    expect(log.tail()).toContain('/api/v1/sessions/link/[redacted]')
    expect(log.tail()).not.toContain('do-not-print')
  })

  /** The product window opens a Sign-In Link, and Electron names the
   *  address in the error of a load that failed. The secret stays good
   *  until the page posts it, so no message that the client shows or
   *  logs holds it. */
  it('redacts the secret of a Sign-In Link', () => {
    const failure = redact(
      "ERR_CONNECTION_REFUSED (-102) loading 'https://pagis-home.tail1234.ts.net/sign-in#do-not-print'",
    )

    expect(failure).toBe("ERR_CONNECTION_REFUSED (-102) loading 'https://pagis-home.tail1234.ts.net/sign-in#[redacted]'")
  })

  it('redacts the Client Credential and the session cookie', () => {
    const log = new DiagnosticLog(200)
    log.append(Buffer.from(`client-credential=${'a'.repeat(64)}\n`))
    log.append(Buffer.from('set-cookie: pagis_session=do-not-print; Path=/\n'))

    expect(log.tail()).toContain('credential=[redacted]')
    expect(log.tail()).toContain('pagis_session=[redacted]')
    expect(log.tail()).not.toContain('a'.repeat(64))
    expect(log.tail()).not.toContain('do-not-print')
  })

  it('does not join streams or leak the suffix of an oversized secret line', () => {
    const log = new DiagnosticLog(200)
    log.append(Buffer.from('credential='), 'stdout')
    log.append(Buffer.from('ordinary stderr\n'), 'stderr')
    log.append(Buffer.from(`hidden-${'x'.repeat(1024 * 1024)}`), 'stdout')
    log.append(Buffer.from('secret-suffix\nstill safe\n'), 'stdout')

    expect(log.tail()).toContain('ordinary stderr')
    expect(log.tail()).toContain('still safe')
    expect(log.tail()).not.toContain('hidden')
    expect(log.tail()).not.toContain('secret-suffix')
  })
  it('spawns the daemon and loads its plain URL', async () => {
    const { home, port } = await newHome()
    const supervisor = newSupervisor(home)

    await supervisor.start()

    expect(supervisor.state).toEqual({
      kind: 'running',
      url: `http://127.0.0.1:${port}/`,
      port,
      version: OUR_VERSION,
      owned: true,
    })
    expect(PidFile.inside(home).read()).toBe(supervisor.pid)
    // The shell starts the daemon again after a restart, and tells it so.
    expect(fs.readFileSync(path.join(home, 'fake-supervised'), 'utf8')).toBe('1')
  })

  it('attaches to a healthy daemon of the same version', async () => {
    const { home, port } = await newHome()
    await startRival(home, port, { FAKE_VERSION: OUR_VERSION })
    const supervisor = newSupervisor(home)

    await supervisor.start()

    expect(supervisor.state).toMatchObject({ kind: 'running', port, owned: false })
    expect(supervisor.pid).toBeNull()
  })

  it('never stops an authenticated server of another version', async () => {
    const { home, port } = await newHome()
    const rival = await startRival(home, port, { FAKE_VERSION: '0.0.9' })
    const supervisor = newSupervisor(home)

    await supervisor.start()

    expect(supervisor.state).toMatchObject({ kind: 'failed' })
    expect(isAlive(rival.pid!)).toBe(true)
  })

  it('advances the launch boundary only when it starts an owned server', async () => {
    const external = await newHome()
    const externalState = new RuntimeState(path.join(external.home, 'runtime'))
    externalState.activate('0.1.0')
    await startRival(external.home, external.port, { FAKE_VERSION: '0.1.0' })
    const blocked = newSupervisor(external.home, {
      version: '0.2.0',
      beforeSpawn: () => externalState.beginLaunch('0.2.0'),
    })

    await blocked.start()

    expect(blocked.state).toMatchObject({ kind: 'failed' })
    expect(externalState.releaseToStart()).toBe('0.1.0')

    const owned = await newHome()
    const ownedState = new RuntimeState(path.join(owned.home, 'runtime'))
    ownedState.activate('0.1.0')
    process.env.FAKE_VERSION = '0.2.0'
    cleanups.push(() => { delete process.env.FAKE_VERSION })
    const started = newSupervisor(owned.home, {
      version: '0.2.0',
      beforeSpawn: () => ownedState.beginLaunch('0.2.0'),
    })

    await started.start()

    expect(started.state).toMatchObject({ kind: 'running', owned: true })
    expect(ownedState.releaseToStart()).toBe('0.2.0')
  })

  it('reports a taken port and starts again on the port the user takes', async () => {
    const { home, port } = await newHome()
    await startRival(home, port, { FAKE_MODE: 'hold' })
    const supervisor = newSupervisor(home)

    void supervisor.start()
    const taken = await waitForState(supervisor, (state) => state.kind === 'taken-port')

    expect(taken).toMatchObject({ kind: 'taken-port', port })
    const suggested = (taken as { suggested: number }).suggested
    expect(suggested).toBeGreaterThan(port)

    await supervisor.usePort(suggested)

    expect(supervisor.state).toMatchObject({
      kind: 'running',
      port: suggested,
      owned: true,
    })
    expect(readPort(home)).toBe(suggested)
  })

  /** The setup page reads the state when the start ends, so the start
   *  ends on the taken-port state and not before it. */
  it('ends the start on a taken port with the holder and the next free port', async () => {
    const { home, port } = await newHome()
    await startRival(home, port, { FAKE_MODE: 'hold' })
    const supervisor = newSupervisor(home)

    await supervisor.start()

    expect(supervisor.state).toMatchObject({ kind: 'taken-port', port })
    expect((supervisor.state as { suggested: number }).suggested).toBeGreaterThan(port)
  })

  /** Port 4400 is taken at the first start, and 4401 is free only
   *  because the daemon that binds it does not run. */
  it('never proposes the Administration Port at the first start', async () => {
    const { home, port, administration } = await newHomeWithAdministrationPort()
    await startRival(home, port, { FAKE_MODE: 'hold' })
    const supervisor = newSupervisor(home)

    await supervisor.start()

    expect(supervisor.state).toMatchObject({ kind: 'taken-port', port })
    const suggested = (supervisor.state as { suggested: number }).suggested
    expect(suggested).toBeGreaterThan(administration)

    await supervisor.usePort(suggested)

    expect(supervisor.state).toMatchObject({ kind: 'running', port: suggested })
    expect(readAdministrationPort(home)).toBe(administration)
  })

  it('never proposes the Administration Port at a later start', async () => {
    const { home, port, administration } = await newHomeWithAdministrationPort()
    const supervisor = newSupervisor(home)
    await supervisor.start()
    expect(supervisor.state).toMatchObject({ kind: 'running', port })
    await supervisor.stop()
    await startRival(home, port, { FAKE_MODE: 'hold' })

    await supervisor.start()

    expect(supervisor.state).toMatchObject({ kind: 'taken-port', port })
    expect((supervisor.state as { suggested: number }).suggested).toBeGreaterThan(administration)
  })

  it('names the Administration Port and its holder when another process holds it', async () => {
    const { home, administration } = await newHomeWithAdministrationPort()
    await holdPort(administration)
    const holder = (await portHolder(administration)) ?? 'another process'
    const supervisor = newSupervisor(home)

    await supervisor.start()

    expect(supervisor.state).toMatchObject({ kind: 'failed' })
    const reason = (supervisor.state as { reason: string }).reason
    expect(reason).toContain(`${holder} uses port ${administration}`)
    expect(reason).toContain('[administration] port')
    expect(reason).not.toContain('stopped')
  })

  it('starts the daemon again when it exits with the restart code', async () => {
    const { home, port } = await newHome()
    const supervisor = newSupervisor(home)
    await supervisor.start()
    const first = supervisor.pid

    await fetch(`http://127.0.0.1:${port}/api/v1/system/restart`, { method: 'POST' })

    await waitForState(
      supervisor,
      (state) => state.kind === 'running' && supervisor.pid !== first,
    )
    expect(supervisor.pid).not.toBeNull()
  })

  it('gives up after three restarts and keeps the log', async () => {
    const { home } = await newHome()
    const supervisor = new DaemonSupervisor({
      home,
      binaryPath: FAKE_DAEMON,
      version: OUR_VERSION,
      computerImage: OUR_IMAGE,
      pidFile: PidFile.inside(home),
      restartPauseMs: 10,
      healthTimeoutMs: 2000,
    })
    cleanups.push(() => supervisor.stop())
    process.env.FAKE_MODE = 'crash'
    cleanups.push(() => {
      delete process.env.FAKE_MODE
    })

    void supervisor.start()
    const failed = await waitForState(supervisor, (state) => state.kind === 'failed')

    expect(failed).toMatchObject({ kind: 'failed' })
    expect((failed as { log: string }).log).toContain('the boot failed')
  })

  it('stops the daemon on quit', async () => {
    const { home } = await newHome()
    const supervisor = newSupervisor(home)
    await supervisor.start()
    const pid = supervisor.pid!

    await supervisor.stop()

    expect(isAlive(pid)).toBe(false)
    expect(PidFile.inside(home).read()).toBeNull()
  })

  it('does not trust or stop a process named by a stale PID record', async () => {
    const { home, port } = await newHome()
    const leftover = await startRival(home, port, { FAKE_VERSION: OUR_VERSION })
    const pidFile = PidFile.inside(home)
    pidFile.write(leftover.pid!)
    const supervisor = newSupervisor(home)

    await supervisor.start()

    expect(supervisor.state).toMatchObject({ kind: 'running', owned: false })
    expect(isAlive(leftover.pid!)).toBe(true)
  })

  it('serializes repeated retries and starts one owned server', async () => {
    const { home } = await newHome()
    const supervisor = newSupervisor(home)

    await Promise.all([supervisor.start(), supervisor.start(), supervisor.start()])

    expect(supervisor.state).toMatchObject({ kind: 'running', owned: true })
    expect(supervisor.pid).not.toBeNull()
  })

  it('stops the exact child when authenticated startup times out', async () => {
    const { home } = await newHome()
    process.env.FAKE_MODE = 'hold'
    cleanups.push(() => { delete process.env.FAKE_MODE })
    const supervisor = new DaemonSupervisor({
      home, binaryPath: FAKE_DAEMON, version: OUR_VERSION, computerImage: OUR_IMAGE,
      pidFile: PidFile.inside(home), restartPauseMs: 20, healthTimeoutMs: 100,
    })
    cleanups.push(() => supervisor.stop())
    const job = supervisor.start()
    while (supervisor.pid === null) await sleep(1)
    const pid = supervisor.pid

    await job

    expect(supervisor.state).toMatchObject({ kind: 'failed' })
    expect(isAlive(pid!)).toBe(false)
  })

  it('does not let a delayed crash restart race with Quit', async () => {
    const { home } = await newHome()
    process.env.FAKE_MODE = 'crash'
    cleanups.push(() => { delete process.env.FAKE_MODE })
    const supervisor = new DaemonSupervisor({
      home, binaryPath: FAKE_DAEMON, version: OUR_VERSION, computerImage: OUR_IMAGE,
      pidFile: PidFile.inside(home), restartPauseMs: 200, healthTimeoutMs: 1000,
    })
    cleanups.push(() => supervisor.stop())

    const job = supervisor.start()
    await sleep(100)
    await supervisor.stop()
    await job
    await sleep(250)

    expect(supervisor.pid).toBeNull()
    expect(PidFile.inside(home).read()).toBeNull()
  })
})

/** A child that ends on the signal the test names, and records each
 *  signal it gets. */
class FakeChild extends EventEmitter {
  exitCode: number | null = null
  signalCode: NodeJS.Signals | null = null
  signals: string[] = []

  constructor(private readonly endsOn: string) {
    super()
  }

  kill(signal: string): boolean {
    this.signals.push(signal)
    if (signal === this.endsOn) {
      this.signalCode = signal as NodeJS.Signals
      this.emit('close', null, signal)
    }
    return true
  }
}

// The daemon stops each Computer with a 10-second `docker stop` on
// SIGINT, so it gets 30 seconds, as the Compose file gives the Headless
// Server, before the client kills it.
describe('the stop of the daemon', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('gives the daemon 30 seconds after SIGINT before it kills it', async () => {
    vi.useFakeTimers()
    const child = new FakeChild('SIGKILL')

    const stopped = stopOwnedChild(child as unknown as ChildProcess)
    expect(child.signals).toEqual(['SIGINT'])
    await vi.advanceTimersByTimeAsync(STOP_GRACE_MS - 1)
    expect(child.signals).toEqual(['SIGINT'])
    await vi.advanceTimersByTimeAsync(1)
    await stopped

    expect(STOP_GRACE_MS).toBe(30000)
    expect(child.signals).toEqual(['SIGINT', 'SIGKILL'])
  })

  it('does not kill a daemon that stops on SIGINT', async () => {
    const child = new FakeChild('SIGINT')

    await stopOwnedChild(child as unknown as ChildProcess)

    expect(child.signals).toEqual(['SIGINT'])
  })
})
