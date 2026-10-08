// The Harness Sign-In of the Client App: the script that runs the vendor's
// own sign-in command in a terminal window, the terminal app that opens
// it, and the exit code that is the only answer.

import { type ChildProcess, spawn } from 'node:child_process'
import { chmodSync, existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { type HarnessSignInRequest, runSignIn, type TerminalLauncher } from './harnessSignIn'

/** A directory that holds an executable `/bin/sh` script for each name. */
function directoryWith(programs: Record<string, string>): string {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'pagis-sign-in-path-'))
  for (const [name, body] of Object.entries(programs)) {
    const file = path.join(directory, name)
    writeFileSync(file, `#!/bin/sh\n${body}\n`)
    chmodSync(file, 0o755)
  }
  return directory
}

function request(command: string, args: string[] = [], env: Record<string, string> = {}): HarnessSignInRequest {
  return { id: 'sign-in-1', harness: 'codex', name: 'Codex', command, args, env }
}

interface Launch {
  program: string
  args: string[]
}

/**
 * A stand-in for the terminal app. It records how the Client App opened
 * it, and runs the script, its last argument, with `/bin/sh` in a process
 * group of its own, as a terminal window does. It keeps all that the
 * window shows, so a test can prove that the answer holds none of it.
 */
class FakeTerminal {
  launches: Launch[] = []
  shown = ''
  private children: ChildProcess[] = []

  constructor(private readonly onShown: (text: string, child: ChildProcess) => void = () => {}) {}

  readonly launch: TerminalLauncher = async (program, args) => {
    this.launches.push({ program, args: [...args] })
    const child = spawn('/bin/sh', [args[args.length - 1]], {
      detached: true,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    this.children.push(child)
    const show = (chunk: Buffer) => {
      this.shown += chunk.toString()
      this.onShown(this.shown, child)
    }
    child.stdout?.on('data', show)
    child.stderr?.on('data', show)
  }

  /** The directory of the script of the last launch. */
  scriptDirectory(): string {
    const args = this.launches[this.launches.length - 1].args
    return path.dirname(args[args.length - 1])
  }

  closeAll(): void {
    for (const child of this.children) {
      if (child.exitCode === null && child.signalCode === null) killGroup(child, 'SIGKILL')
    }
  }
}

function killGroup(child: ChildProcess, signal: NodeJS.Signals): void {
  try {
    process.kill(-(child.pid as number), signal)
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error
  }
}

const SYSTEM_PATH = '/usr/bin:/bin'

describe('a Harness Sign-In in a terminal window', () => {
  let terminal: FakeTerminal

  afterEach(() => {
    terminal?.closeAll()
    vi.useRealTimers()
  })

  it('answers the exit code of the vendor program, and removes its directory', async () => {
    terminal = new FakeTerminal()

    const answer = await runSignIn(request('sh', ['-c', 'exit 3']), {
      platform: 'darwin',
      environment: async () => ({ PATH: SYSTEM_PATH }),
      launch: terminal.launch,
    })

    expect(answer).toEqual({ exit_code: 3 })
    expect(existsSync(terminal.scriptDirectory())).toBe(false)
    expect(terminal.shown).toContain(
      'Pagis opened this window to sign in to Codex. Pagis does not read what you type or what this window shows.',
    )
  })

  it('gives the program its arguments as they are, the env of the request and the login-shell PATH', async () => {
    terminal = new FakeTerminal()
    const record = path.join(mkdtempSync(path.join(os.tmpdir(), 'pagis-sign-in-record-')), 'record')
    const programs = directoryWith({
      'vendor-login': `printf '%s\\n' "$@" > "$RECORD"\nprintf 'mode=%s\\npath=%s\\n' "$VENDOR_MODE" "$PATH" >> "$RECORD"`,
    })
    const loginPath = `${programs}:${SYSTEM_PATH}`
    const args = ['plain', 'two words', "it's", '$HOME', '`id`', '"quoted"']

    const answer = await runSignIn(
      request('vendor-login', args, { RECORD: record, VENDOR_MODE: "console 'login' $USER" }),
      { platform: 'darwin', environment: async () => ({ PATH: loginPath }), launch: terminal.launch },
    )

    expect(answer).toEqual({ exit_code: 0 })
    expect(readFileSync(record, 'utf8')).toBe(
      [...args, "mode=console 'login' $USER", `path=${loginPath}`, ''].join('\n'),
    )
  })

  /** A terminal app sends a signal to the process group of the window
   *  that the person closes. */
  it('answers 143 when the window gets a TERM', async () => {
    terminal = new FakeTerminal((shown, child) => {
      if (shown.includes('ready')) killGroup(child, 'SIGTERM')
    })

    const answer = await runSignIn(request('sh', ['-c', 'echo ready; sleep 30']), {
      platform: 'darwin',
      environment: async () => ({ PATH: SYSTEM_PATH }),
      launch: terminal.launch,
    })

    expect(answer).toEqual({ exit_code: 143 })
  })

  it('answers no exit code when the sign-in does not end in 30 minutes', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    const launches: Launch[] = []
    let launched!: () => void
    const opened = new Promise<void>((resolve) => {
      launched = resolve
    })

    const answer = runSignIn(request('sh', ['-c', 'sleep 30']), {
      platform: 'darwin',
      environment: async () => ({ PATH: SYSTEM_PATH }),
      launch: async (program, args) => {
        launches.push({ program, args: [...args] })
        launched()
      },
    })
    await opened
    await vi.advanceTimersByTimeAsync(30 * 60 * 1000)

    expect(await answer).toEqual({ exit_code: null, error: 'The sign-in did not end in 30 minutes' })
    expect(existsSync(path.dirname(launches[0].args[2]))).toBe(false)
  })

  it('answers no byte that the program wrote to stdout or stderr', async () => {
    terminal = new FakeTerminal()

    const answer = await runSignIn(
      request('sh', ['-c', 'echo sk-secret-out; echo sk-secret-err >&2; exit 1']),
      { platform: 'darwin', environment: async () => ({ PATH: SYSTEM_PATH }), launch: terminal.launch },
    )

    expect(terminal.shown).toContain('sk-secret-out')
    expect(terminal.shown).toContain('sk-secret-err')
    expect(answer).toEqual({ exit_code: 1 })
    expect(JSON.stringify(answer)).not.toContain('sk-secret')
  })

  /** A name of the env is written into the script, so a name that is
   *  not a shell variable name could run a command of its own. */
  it('refuses an env variable whose name is not a shell variable name', async () => {
    terminal = new FakeTerminal()

    const answer = await runSignIn(request('true', [], { 'A;touch /tmp/x': '1' }), {
      platform: 'darwin',
      environment: async () => ({ PATH: SYSTEM_PATH }),
      launch: terminal.launch,
    })

    expect(answer.exit_code).toBe(null)
    expect(answer.error).toContain('A;touch /tmp/x')
    expect(terminal.launches).toEqual([])
  })
})

describe('the terminal app of a Harness Sign-In', () => {
  let terminal: FakeTerminal

  afterEach(() => {
    terminal?.closeAll()
  })

  it('opens the script in Terminal on macOS', async () => {
    terminal = new FakeTerminal()

    await runSignIn(request('true'), {
      platform: 'darwin',
      environment: async () => ({ PATH: SYSTEM_PATH }),
      launch: terminal.launch,
    })

    expect(terminal.launches).toHaveLength(1)
    const { program, args } = terminal.launches[0]
    expect(program).toBe('open')
    expect(args.slice(0, 2)).toEqual(['-a', 'Terminal'])
    expect(args).toHaveLength(3)
    expect(path.basename(args[2])).toBe('sign-in.command')
  })

  it.each([
    ['x-terminal-emulator', ['-e']],
    ['gnome-terminal', ['--']],
    ['konsole', ['-e']],
    ['xterm', ['-e']],
  ])('opens the script with %s, the first terminal app on the PATH of Linux', async (launcher, form) => {
    terminal = new FakeTerminal()
    // xterm is the last choice, so it is on the PATH as well to prove the order.
    const launchers = directoryWith({ [launcher]: 'exit 0', xterm: 'exit 0' })

    await runSignIn(request('true'), {
      platform: 'linux',
      environment: async () => ({ PATH: `${launchers}:${SYSTEM_PATH}` }),
      launch: terminal.launch,
    })

    expect(terminal.launches).toHaveLength(1)
    const { program, args } = terminal.launches[0]
    expect(program).toBe(path.join(launchers, launcher))
    expect(args.slice(0, -1)).toEqual(form)
    expect(path.basename(args[args.length - 1])).toBe('sign-in.command')
  })

  it('answers an error when Linux has no terminal app on the PATH', async () => {
    terminal = new FakeTerminal()

    const answer = await runSignIn(request('true'), {
      platform: 'linux',
      environment: async () => ({ PATH: directoryWith({}) }),
      launch: terminal.launch,
    })

    expect(answer).toEqual({ exit_code: null, error: 'No terminal app was found' })
    expect(terminal.launches).toEqual([])
  })
})
