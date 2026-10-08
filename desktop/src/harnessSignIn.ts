// The Harness Sign-In of a Coding Harness on this machine (ADR-0033).
//
// The daemon asks the Client App to run the vendor's own sign-in command,
// in the vendor's own program. The Client App writes a short script and
// opens it in the terminal app of the platform. The person signs in there,
// so the terminal app and the vendor's program are the only processes
// that see what the person types and what the window shows. The Client
// App reads only the exit code that the script writes to a file. The
// script holds the command and the env that the daemon sent, and neither
// is a secret.
//
// The exit code tells how the vendor's program ended, not that the person
// signed in.
//
// This module imports only Node built-ins and `./loginShell`.

import { spawn } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'

import { findOnPath, loginShellEnvironment } from './loginShell'

/** One Harness Sign-In that the daemon asked for. */
export interface HarnessSignInRequest {
  id: string
  /** The id of the harness in the Harness Catalog. */
  harness: string
  /** The name of the harness that the person reads. */
  name: string
  /** The vendor's program, as the ACP terminal method or the Harness
   *  Catalog names it. */
  command: string
  args: readonly string[]
  env: Readonly<Record<string, string>>
}

/** How the sign-in ended. `exit_code` is null when the vendor's program
 *  did not give one, and `error` then tells why. */
export interface HarnessSignInResult {
  exit_code: number | null
  error?: string
}

/** Starts the terminal app `program` with `args`. It resolves when the
 *  program started and rejects when it did not. */
export type TerminalLauncher = (program: string, args: readonly string[]) => Promise<void>

export interface SignInDeps {
  /** The platform of this machine. */
  platform?: NodeJS.Platform
  /** The environment of the person's login shell. Only its PATH is used. */
  environment?: () => Promise<Record<string, string>>
  launch?: TerminalLauncher
}

/** Runs one Harness Sign-In and answers how it ended. */
export type SignInRunner = (request: HarnessSignInRequest) => Promise<HarnessSignInResult>

const SCRIPT_NAME = 'sign-in.command'
const EXIT_NAME = 'exit'
const POLL_MS = 1_000
const DEADLINE_MS = 30 * 60 * 1_000

/** The terminal apps of Linux in the order of choice, each with the
 *  arguments that come before the script. `x-terminal-emulator` is the
 *  Debian alternative. The others cover the desktops without it. */
const LINUX_TERMINALS: readonly (readonly [string, readonly string[]])[] = [
  ['x-terminal-emulator', ['-e']],
  ['gnome-terminal', ['--']],
  ['konsole', ['-e']],
  ['xterm', ['-e']],
]

const VARIABLE_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/

/**
 * The POSIX `sh` script of a sign-in.
 *
 * It exports the login-shell `PATH` and the env of the request, prints one
 * line that tells the person what the window is, and runs the vendor's
 * program. It writes the exit code to the file `exit` beside the script.
 * A window that the person closes sends a signal, and the trap then writes
 * 128 plus the number of the signal. The code goes to `exit.part` first,
 * so the Client App never reads half of it.
 *
 * It throws for a name of the env that is not a shell variable name,
 * because the name goes into the script as it is.
 */
export function signInScript(request: HarnessSignInRequest, pathValue: string): string {
  const exports = [`export PATH=${quote(pathValue)}`]
  for (const [name, value] of Object.entries(request.env)) {
    if (!VARIABLE_NAME.test(name)) {
      throw new Error(`the env of the sign-in names a variable that is not a shell variable name: ${name}`)
    }
    exports.push(`export ${name}=${quote(value)}`)
  }
  const notice = `Pagis opened this window to sign in to ${request.name}. Pagis does not read what you type or what this window shows.`
  return [
    '#!/bin/sh',
    `pagis_directory=$(dirname -- "$0")`,
    'pagis_finish() {',
    `  printf '%s\\n' "$1" > "$pagis_directory/${EXIT_NAME}.part"`,
    `  mv -f "$pagis_directory/${EXIT_NAME}.part" "$pagis_directory/${EXIT_NAME}"`,
    '  exit "$1"',
    '}',
    "trap 'pagis_finish 129' HUP",
    "trap 'pagis_finish 130' INT",
    "trap 'pagis_finish 143' TERM",
    ...exports,
    `printf '%s\\n' ${quote(notice)}`,
    [request.command, ...request.args].map(quote).join(' '),
    'pagis_finish "$?"',
    '',
  ].join('\n')
}

/**
 * Run one Harness Sign-In in a terminal window and answer its exit code.
 *
 * It writes the script to a new directory that only the OS user can read,
 * opens it in the terminal app, and reads the `exit` file once each
 * second. It answers no exit code when no terminal app is found, when the
 * terminal app does not start, or when the sign-in does not end in 30
 * minutes. It removes the directory when it answers. It never rejects.
 */
export async function runSignIn(request: HarnessSignInRequest, deps: SignInDeps = {}): Promise<HarnessSignInResult> {
  const platform = deps.platform ?? process.platform
  const environment = deps.environment ?? (() => loginShellEnvironment())
  const launch = deps.launch ?? launchDetached

  let directory: string | null = null
  try {
    const pathValue = (await environment()).PATH ?? ''
    const script = signInScript(request, pathValue)
    const terminal = await terminalCommand(platform, pathValue)
    if (terminal === null) return { exit_code: null, error: 'No terminal app was found' }

    directory = await mkdtemp(path.join(os.tmpdir(), 'pagis-sign-in-'))
    const file = path.join(directory, SCRIPT_NAME)
    await writeFile(file, script, { mode: 0o700 })

    const ended = waitForExit(path.join(directory, EXIT_NAME))
    try {
      await launch(terminal.program, [...terminal.args, file])
    } catch (error) {
      ended.stop()
      throw error
    }
    return await ended.result
  } catch (error) {
    return { exit_code: null, error: (error as Error).message }
  } finally {
    if (directory !== null) await rm(directory, { recursive: true, force: true })
  }
}

/** The terminal app that opens the script, or null when there is none. */
async function terminalCommand(
  platform: NodeJS.Platform,
  pathValue: string,
): Promise<{ program: string; args: readonly string[] } | null> {
  // Terminal runs a `.command` file in a new window and comes to the
  // front. AppleScript would need the Apple Events entitlement and a
  // consent prompt.
  if (platform === 'darwin') return { program: 'open', args: ['-a', 'Terminal'] }
  for (const [name, args] of LINUX_TERMINALS) {
    const program = await findOnPath(name, pathValue)
    if (program !== null) return { program, args }
  }
  return null
}

/** Read the exit file once each second until it holds a code or the
 *  deadline comes. `stop` ends the wait with no answer. */
function waitForExit(file: string): { result: Promise<HarnessSignInResult>; stop: () => void } {
  let done = false
  let poll: ReturnType<typeof setTimeout> | undefined
  let deadline: ReturnType<typeof setTimeout> | undefined
  const stop = () => {
    done = true
    clearTimeout(poll)
    clearTimeout(deadline)
  }
  const result = new Promise<HarnessSignInResult>((resolve) => {
    const answer = (value: HarnessSignInResult) => {
      if (done) return
      stop()
      resolve(value)
    }
    const check = async () => {
      let code: number | null
      try {
        code = await readExitCode(file)
      } catch (error) {
        answer({ exit_code: null, error: `the exit code of the sign-in was not read: ${(error as Error).message}` })
        return
      }
      if (code !== null) answer({ exit_code: code })
      else if (!done) poll = setTimeout(check, POLL_MS)
    }
    deadline = setTimeout(() => answer({ exit_code: null, error: 'The sign-in did not end in 30 minutes' }), DEADLINE_MS)
    poll = setTimeout(check, POLL_MS)
  })
  return { result, stop }
}

/** The code in the exit file, or null while there is no file. */
async function readExitCode(file: string): Promise<number | null> {
  let text: string
  try {
    text = await readFile(file, 'utf8')
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return null
    throw error
  }
  const code = Number.parseInt(text.trim(), 10)
  if (Number.isNaN(code)) throw new Error(`the exit file holds no number: ${JSON.stringify(text.slice(0, 20))}`)
  return code
}

/** Start the terminal app apart from the Client App, so that it stays
 *  open when the Client App quits. */
const launchDetached: TerminalLauncher = (program, args) =>
  new Promise((resolve, reject) => {
    const child = spawn(program, args, { detached: true, stdio: 'ignore' })
    child.once('error', (error) => reject(new Error(`the terminal app ${program} did not start: ${error.message}`)))
    child.once('spawn', () => {
      child.unref()
      resolve()
    })
  })

/** Quote a word for a POSIX shell. */
function quote(word: string): string {
  return `'${word.replace(/'/g, `'\\''`)}'`
}
