// The check of the sign-in state of a Coding Harness: the vendor's own
// status command, of which the Client App reads only the exit code and
// whether the output holds fixed words.

import { chmodSync, mkdtempSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'

import { describe, expect, it } from 'vitest'

import { readSignInState, runSignInCheck, type SignInCheck } from './harnessSignInCheck'

/** A directory that holds an executable `/bin/sh` script for each name. */
function directoryWith(programs: Record<string, string>): string {
  const directory = mkdtempSync(path.join(os.tmpdir(), 'pagis-check-path-'))
  for (const [name, body] of Object.entries(programs)) {
    const file = path.join(directory, name)
    writeFileSync(file, `#!/bin/sh\n${body}\n`)
    chmodSync(file, 0o755)
  }
  return directory
}

const CODEX: SignInCheck = { command: ['codex', 'login', 'status'], signed_out: 'Not logged in' }

function on(pathValue: string) {
  return { environment: async () => ({ PATH: pathValue, HOME: os.tmpdir() }) }
}

describe('the sign-in state of a status command', () => {
  it.each([
    [0, 'Logged in using ChatGPT\n', 'signed_in'],
    [1, 'Not logged in\n', 'not_signed_in'],
    // The Cursor CLI exits with 0 when the Person is not signed in.
    [0, 'Not logged in\n', 'not_signed_in'],
    [1, 'Error loading configuration\n', 'unknown'],
    [null, '', 'unknown'],
  ] as const)('reads the exit code %s and the output %j as %s', (code, output, state) => {
    expect(readSignInState(code, output, 'Not logged in')).toBe(state)
  })
})

describe('a check on this machine', () => {
  it('runs the command on the PATH of the login shell and answers signed in for exit code 0', async () => {
    const pathValue = directoryWith({ codex: 'echo "Logged in using an API key - sk-proj-***ABCDE"' })

    expect(await runSignInCheck(CODEX, on(pathValue))).toBe('signed_in')
  })

  it('answers not signed in when the output holds the fixed words, on stderr too', async () => {
    const pathValue = directoryWith({ codex: 'echo "Not logged in" >&2\nexit 1' })

    expect(await runSignInCheck(CODEX, on(pathValue))).toBe('not_signed_in')
  })

  it('passes the arguments of the catalog to the program', async () => {
    const pathValue = directoryWith({ codex: '[ "$1 $2" = "login status" ] || exit 9\nexit 0' })

    expect(await runSignInCheck(CODEX, on(pathValue))).toBe('signed_in')
  })

  it('answers unknown when the program is not on the PATH', async () => {
    expect(await runSignInCheck(CODEX, on(directoryWith({})))).toBe('unknown')
  })

  it('answers unknown when the login shell fails', async () => {
    const failed = {
      environment: async () => {
        throw new Error('the login shell /bin/zsh exited with code 1')
      },
    }

    expect(await runSignInCheck(CODEX, failed)).toBe('unknown')
  })

  it('stops a command that does not end at the deadline and answers unknown', async () => {
    const pathValue = directoryWith({ codex: 'sleep 10' })

    const started = Date.now()
    expect(await runSignInCheck(CODEX, { ...on(pathValue), timeoutMs: 200 })).toBe('unknown')
    expect(Date.now() - started).toBeLessThan(5_000)
  })

  it('gives the command no input, so a command that asks a question ends', async () => {
    const pathValue = directoryWith({ codex: 'read answer || exit 3\nexit 0' })

    expect(await runSignInCheck(CODEX, on(pathValue))).toBe('unknown')
  })
})
