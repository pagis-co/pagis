// The environment of the person's login shell, and the search of a
// command on its PATH.
//
// A macOS app that the Dock or the Finder starts gets the environment of
// launchd, and a Linux app that the desktop starts does not read
// `~/.bashrc`. So `node` from nvm, `git` from Homebrew and a vendor CLI in
// `~/.local/bin` are not on the PATH of the Client App. This module runs
// the person's login shell, as VS Code does to resolve the shell
// environment, and reads the environment that the startup files make.
//
// The module gives the whole environment, not only PATH, so a Coding
// Harness runs as it runs in the person's terminal. The variables stay on
// this machine. A failure is an error: the environment of the Client App
// in its place would hide why a command is not found.
//
// This module uses only erasable TypeScript syntax and imports only Node
// built-ins, so plain `node` loads it in an interop test of the daemon.

import { spawn } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import { constants } from 'node:fs'
import { access, stat } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'

/** One run of the login shell: the shell, its arguments, its
 *  environment, and the deadline in milliseconds. */
export interface ShellRun {
  shell: string
  args: readonly string[]
  env: Record<string, string>
  timeoutMs: number
}

/** What a run of the login shell gave. `timedOut` is true when the runner
 *  stopped the shell at the deadline. */
export interface ShellResult {
  code: number | null
  stdout: string
  stderr: string
  timedOut: boolean
}

/** Run a login shell to its end or to its deadline. It rejects only when
 *  the shell does not start. */
export type ShellRunner = (run: ShellRun) => Promise<ShellResult>

export interface LoginShellOptions {
  /** The login shell. The default is the entry of the user database. */
  shell?: string
  /** The executable that runs as Node and prints the environment. */
  executable?: string
  /** The deadline of the shell in milliseconds. */
  timeoutMs?: number
  run?: ShellRunner
}

const DEFAULT_TIMEOUT_MS = 10_000

// Oh My Zsh asks whether to update and can start tmux. Both stop a shell
// that has no terminal. The `shell-env` package sets the same variables.
const SHELL_VARIABLES: Record<string, string> = {
  DISABLE_AUTO_UPDATE: 'true',
  ZSH_TMUX_AUTOSTARTED: 'true',
  ZSH_TMUX_AUTOSTART: 'false',
}

/**
 * Read the environment of the person's login shell.
 *
 * It runs the shell with `-i -l -c`, so the shell reads the same startup
 * files as a terminal. The command runs the executable of the Client App
 * as Node, which prints its environment as JSON between two random
 * markers. All other output of the startup files is ignored.
 *
 * It reads the environment again at each call and keeps no copy, so a
 * change of the startup files applies to the next call.
 */
export async function loginShellEnvironment(options: LoginShellOptions = {}): Promise<Record<string, string>> {
  const shell = options.shell ?? os.userInfo().shell
  if (!shell) {
    throw new Error('the user database names no login shell for this user')
  }
  const executable = options.executable ?? process.execPath
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS
  const run = options.run ?? runShell

  const marker = randomBytes(12).toString('hex')
  const script = `process.stdout.write("${marker}" + JSON.stringify(process.env) + "${marker}")`
  const env: Record<string, string> = {
    ...definedVariables(process.env),
    ...SHELL_VARIABLES,
    ELECTRON_RUN_AS_NODE: '1',
  }

  let result: ShellResult
  try {
    result = await run({
      shell,
      args: ['-i', '-l', '-c', `${quote(executable)} -e ${quote(script)}`],
      env,
      timeoutMs,
    })
  } catch (error) {
    throw new Error(`the login shell ${shell} did not start: ${(error as Error).message}`)
  }

  const detail = lastLine(result.stderr)
  if (result.timedOut) {
    throw new Error(`the login shell ${shell} did not finish in ${timeoutMs / 1000} seconds${detail}`)
  }
  if (result.code !== 0) {
    throw new Error(`the login shell ${shell} exited with code ${result.code}${detail}`)
  }
  // JSON.stringify writes a line feed in a value as `\n`, so the JSON is
  // on one line. The braces keep an echo of the command itself out.
  const match = new RegExp(`${marker}(\\{.*\\})${marker}`).exec(result.stdout)
  if (match === null) {
    throw new Error(`the login shell ${shell} printed no environment${detail}`)
  }

  const environment = JSON.parse(match[1]) as Record<string, string>
  delete environment.ELECTRON_RUN_AS_NODE
  for (const name of Object.keys(SHELL_VARIABLES)) {
    delete environment[name]
  }
  return environment
}

/**
 * Give the absolute path of the executable file that `command` names, or
 * null when there is none.
 *
 * A command with a `/` is checked as it is. Another command is checked in
 * each absolute directory of `pathValue`, in order.
 */
export async function findOnPath(command: string, pathValue: string): Promise<string | null> {
  if (command.includes('/')) {
    return (await isExecutableFile(command)) ? command : null
  }
  for (const directory of pathValue.split(path.delimiter)) {
    // An empty or a relative entry names a directory relative to the
    // current directory, which means nothing to the Client App.
    if (!path.isAbsolute(directory)) {
      continue
    }
    const candidate = path.join(directory, command)
    if (await isExecutableFile(candidate)) {
      return candidate
    }
  }
  return null
}

async function isExecutableFile(file: string): Promise<boolean> {
  try {
    await access(file, constants.X_OK)
    return (await stat(file)).isFile()
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code
    if (code === 'ENOENT' || code === 'EACCES' || code === 'ENOTDIR' || code === 'ELOOP') {
      return false
    }
    throw error
  }
}

/**
 * Run the shell with no terminal, in its own session, so an interactive
 * shell does not take the terminal of the Client App. At the deadline it
 * kills the whole process group, so a command that a startup file started
 * does not hold the pipes open.
 */
const runShell: ShellRunner = (run) =>
  new Promise((resolve, reject) => {
    const child = spawn(run.shell, run.args, {
      env: run.env,
      cwd: os.homedir(),
      detached: true,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    let stdout = ''
    let stderr = ''
    let timedOut = false
    child.stdout.setEncoding('utf8').on('data', (chunk: string) => {
      stdout += chunk
    })
    child.stderr.setEncoding('utf8').on('data', (chunk: string) => {
      stderr += chunk
    })
    const timer = setTimeout(() => {
      timedOut = true
      killGroup(child.pid)
    }, run.timeoutMs)
    child.on('error', (error) => {
      clearTimeout(timer)
      reject(error)
    })
    child.on('close', (code) => {
      clearTimeout(timer)
      resolve({ code, stdout, stderr, timedOut })
    })
  })

function killGroup(pid: number | undefined) {
  if (pid === undefined) {
    return
  }
  try {
    process.kill(-pid, 'SIGKILL')
  } catch (error) {
    // The group is gone when the shell ended at the same time.
    if ((error as NodeJS.ErrnoException).code !== 'ESRCH') {
      throw error
    }
  }
}

/** Quote a word for a POSIX shell, zsh and fish. */
function quote(word: string): string {
  return `'${word.replace(/'/g, `'\\''`)}'`
}

function definedVariables(environment: NodeJS.ProcessEnv): Record<string, string> {
  const variables: Record<string, string> = {}
  for (const [name, value] of Object.entries(environment)) {
    if (value !== undefined) {
      variables[name] = value
    }
  }
  return variables
}

/** The last line of standard error that has text, as the end of a
 *  message, or nothing. */
function lastLine(stderr: string): string {
  const lines = stderr.split('\n').filter((line) => line.trim() !== '')
  return lines.length === 0 ? '' : `: ${lines[lines.length - 1].trim()}`
}
