// The check of the sign-in state of a Coding Harness on this machine
// (ADR-0033).
//
// The daemon names the vendor's own status command of each harness in the
// answer to the registration, for example `codex login status`. The Client
// App runs it with no terminal window and no input. It reads only the exit
// code and whether the output holds the fixed words of a person who is not
// signed in. The output can name the account or show part of a key, so the
// Client App keeps none of it, writes none of it to the log, and sends only
// the state.
//
// This module imports only Node built-ins and `./loginShell`.

import { spawn } from 'node:child_process'
import os from 'node:os'

import { loginShellEnvironment } from './loginShell'

/** The status command of one harness, as the daemon names it. */
export interface SignInCheck {
  /** The argument vector. */
  command: readonly string[]
  /** The fixed words that the command writes when the person is not
   *  signed in. */
  signed_out: string
}

/** What a check showed. */
export type SignInState = 'signed_in' | 'not_signed_in' | 'unknown'

/** Runs one check and answers the state. It never rejects. */
export type SignInChecker = (check: SignInCheck) => Promise<SignInState>

export interface CheckDeps {
  /** The environment of the person's login shell. */
  environment?: () => Promise<Record<string, string>>
  /** The deadline of the command in milliseconds. */
  timeoutMs?: number
}

/** The first start of an npx command on a machine downloads the package. */
const DEFAULT_TIMEOUT_MS = 60_000

/** The most output that the check reads, in characters. The fixed words
 *  come at the start of the output of each status command. */
const MAX_OUTPUT = 64 * 1024

/**
 * The state that the end of a status command tells. Output that holds
 * `signedOut` tells that the person is not signed in, whatever the exit
 * code. Else the exit code 0 tells that the person is signed in, and any
 * other end tells nothing.
 */
export function readSignInState(exitCode: number | null, output: string, signedOut: string): SignInState {
  if (output.includes(signedOut)) return 'not_signed_in'
  if (exitCode === 0) return 'signed_in'
  return 'unknown'
}

/** Run the status command of one harness in the environment of the
 *  person's login shell, and answer the state. */
export async function runSignInCheck(check: SignInCheck, deps: CheckDeps = {}): Promise<SignInState> {
  const environment = deps.environment ?? (() => loginShellEnvironment())
  const [program, ...args] = check.command
  if (program === undefined) return 'unknown'
  let env: Record<string, string>
  try {
    env = await environment()
  } catch {
    return 'unknown'
  }
  return new Promise((resolve) => {
    let output = ''
    const child = spawn(program, args, {
      env,
      cwd: os.homedir(),
      stdio: ['ignore', 'pipe', 'pipe'],
      timeout: deps.timeoutMs ?? DEFAULT_TIMEOUT_MS,
    })
    const read = (chunk: Buffer) => {
      if (output.length < MAX_OUTPUT) output += chunk.toString()
    }
    child.stdout?.on('data', read)
    child.stderr?.on('data', read)
    child.once('error', () => resolve('unknown'))
    child.once('close', (code) => resolve(readSignInState(code, output, check.signed_out)))
  })
}
