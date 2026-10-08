// The environment of the person's login shell, and the search of a
// command on a PATH.

import { execFileSync } from 'node:child_process'
import { chmodSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { findOnPath, loginShellEnvironment, type ShellRun, type ShellRunner } from './loginShell'

/** A runner that runs the command of the login shell under `/bin/sh`,
 *  with the environment the module gives, and wraps its output in the
 *  text that startup files print. It keeps the shell and its arguments. */
function fakeShell(before = '', after = ''): ShellRunner & { runs: ShellRun[] } {
  const runs: ShellRun[] = []
  const runner = async (run: ShellRun) => {
    runs.push(run)
    const stdout = execFileSync('/bin/sh', ['-c', run.args[run.args.length - 1]], {
      env: run.env,
      encoding: 'utf8',
    })
    return { code: 0, stdout: `${before}${stdout}${after}`, stderr: '', timedOut: false }
  }
  return Object.assign(runner, { runs })
}

describe('loginShellEnvironment', () => {
  afterEach(() => {
    vi.unstubAllEnvs()
  })

  it('gives the environment between the markers and ignores the text of the startup files', async () => {
    vi.stubEnv('PAGIS_TEST_VALUE', 'from-the-shell')
    const run = fakeShell('Welcome to zsh\nlast login: today\n', '\nbye\n')

    const environment = await loginShellEnvironment({ shell: '/bin/zsh', run })

    expect(environment.PAGIS_TEST_VALUE).toBe('from-the-shell')
    expect(environment.HOME).toBe(process.env.HOME)
  })

  it('gives a value that holds a line feed or an equals sign whole', async () => {
    vi.stubEnv('PAGIS_TEST_LINES', 'one\ntwo\n')
    vi.stubEnv('PAGIS_TEST_EQUALS', 'a=b==c')

    const environment = await loginShellEnvironment({ shell: '/bin/zsh', run: fakeShell() })

    expect(environment.PAGIS_TEST_LINES).toBe('one\ntwo\n')
    expect(environment.PAGIS_TEST_EQUALS).toBe('a=b==c')
  })

  it('removes ELECTRON_RUN_AS_NODE and the variables it sets for the shell', async () => {
    const environment = await loginShellEnvironment({ shell: '/bin/zsh', run: fakeShell() })

    expect(environment).not.toHaveProperty('ELECTRON_RUN_AS_NODE')
    expect(environment).not.toHaveProperty('DISABLE_AUTO_UPDATE')
    expect(environment).not.toHaveProperty('ZSH_TMUX_AUTOSTARTED')
    expect(environment).not.toHaveProperty('ZSH_TMUX_AUTOSTART')
  })

  it('runs the shell as an interactive login shell that Oh My Zsh does not stop', async () => {
    const run = fakeShell()

    await loginShellEnvironment({ shell: '/bin/zsh', executable: process.execPath, timeoutMs: 1234, run })

    expect(run.runs).toHaveLength(1)
    const [shellRun] = run.runs
    expect(shellRun.shell).toBe('/bin/zsh')
    expect(shellRun.args.slice(0, 3)).toEqual(['-i', '-l', '-c'])
    expect(shellRun.args).toHaveLength(4)
    expect(shellRun.args[3]).toContain(process.execPath)
    expect(shellRun.timeoutMs).toBe(1234)
    expect(shellRun.env).toMatchObject({
      ELECTRON_RUN_AS_NODE: '1',
      DISABLE_AUTO_UPDATE: 'true',
      ZSH_TMUX_AUTOSTARTED: 'true',
      ZSH_TMUX_AUTOSTART: 'false',
    })
  })

  it('stops the shell after 10 seconds when no deadline is given', async () => {
    const run = fakeShell()

    await loginShellEnvironment({ shell: '/bin/zsh', run })

    expect(run.runs[0].timeoutMs).toBe(10_000)
  })

  it('quotes an executable whose path holds a space or a quote', async () => {
    const directory = mkdtempSync(path.join(os.tmpdir(), "pagis-app's dir "))
    const executable = path.join(directory, 'Pagis Client')
    writeFileSync(executable, `#!/bin/sh\nexec '${process.execPath}' "$@"\n`)
    chmodSync(executable, 0o755)

    const environment = await loginShellEnvironment({ shell: '/bin/zsh', executable, run: fakeShell() })

    expect(environment.HOME).toBe(process.env.HOME)
  })

  it('rejects with the shell and its last line of standard error when the shell fails', async () => {
    const run: ShellRunner = async () => ({
      code: 1,
      stdout: '',
      stderr: 'first line\n/Users/p/.zshrc:12: command not found: nvm\n',
      timedOut: false,
    })

    await expect(loginShellEnvironment({ shell: '/bin/zsh', run })).rejects.toThrow(
      /\/bin\/zsh.*code 1.*command not found: nvm$/,
    )
  })

  it('rejects with the shell when the shell prints no markers', async () => {
    const run: ShellRunner = async () => ({
      code: 0,
      stdout: '{"PATH":"/usr/bin"}\n',
      stderr: 'zsh: no such file\n',
      timedOut: false,
    })

    await expect(loginShellEnvironment({ shell: '/bin/zsh', run })).rejects.toThrow(
      /\/bin\/zsh.*no environment.*zsh: no such file$/,
    )
  })

  it('rejects with the shell when the shell runs past the deadline', async () => {
    const run: ShellRunner = async () => ({ code: null, stdout: '', stderr: 'waiting\n', timedOut: true })

    await expect(loginShellEnvironment({ shell: '/bin/zsh', timeoutMs: 500, run })).rejects.toThrow(
      /\/bin\/zsh.*0\.5 seconds.*waiting$/,
    )
  })

  it('rejects with the shell when the shell does not start', async () => {
    await expect(loginShellEnvironment({ shell: '/nonexistent/pagis-shell' })).rejects.toThrow(
      /\/nonexistent\/pagis-shell.*did not start/,
    )
  })

  it('stops a real shell at the deadline', async () => {
    const home = mkdtempSync(path.join(os.tmpdir(), 'pagis-login-'))
    writeFileSync(path.join(home, '.profile'), 'sleep 30\n')
    vi.stubEnv('HOME', home)
    vi.stubEnv('ENV', '')

    const started = Date.now()
    await expect(loginShellEnvironment({ shell: '/bin/sh', timeoutMs: 300 })).rejects.toThrow(
      /\/bin\/sh.*did not finish/,
    )
    expect(Date.now() - started).toBeLessThan(5_000)
  })

  it('reads a variable that the .profile of a real /bin/sh exports', async () => {
    const home = mkdtempSync(path.join(os.tmpdir(), 'pagis-login-'))
    writeFileSync(
      path.join(home, '.profile'),
      'echo "a startup file talks"\nexport PAGIS_LOGIN_PROFILE="from .profile"\n',
    )
    vi.stubEnv('HOME', home)
    vi.stubEnv('ENV', '')

    const environment = await loginShellEnvironment({ shell: '/bin/sh' })

    expect(environment.PAGIS_LOGIN_PROFILE).toBe('from .profile')
    expect(environment).not.toHaveProperty('ELECTRON_RUN_AS_NODE')
  })
})

describe('findOnPath', () => {
  function directories() {
    const root = mkdtempSync(path.join(os.tmpdir(), 'pagis-path-'))
    const first = path.join(root, 'first')
    const second = path.join(root, 'second')
    mkdirSync(first)
    mkdirSync(second)
    return { root, first, second }
  }

  function executable(file: string) {
    writeFileSync(file, '#!/bin/sh\n')
    chmodSync(file, 0o755)
  }

  it('finds a command in the second directory of the PATH', async () => {
    const { first, second } = directories()
    executable(path.join(second, 'claude'))

    expect(await findOnPath('claude', [first, second].join(path.delimiter))).toBe(path.join(second, 'claude'))
  })

  it('skips a file that is not executable and a directory with the name of the command', async () => {
    const { root, first, second } = directories()
    const third = path.join(root, 'third')
    mkdirSync(third)
    writeFileSync(path.join(first, 'codex'), 'not executable\n')
    chmodSync(path.join(first, 'codex'), 0o644)
    mkdirSync(path.join(second, 'codex'))
    executable(path.join(third, 'codex'))

    expect(await findOnPath('codex', [first, second, third].join(path.delimiter))).toBe(path.join(third, 'codex'))
  })

  it('checks a command with a slash as it is', async () => {
    const { first, second } = directories()
    executable(path.join(first, 'gemini'))
    executable(path.join(second, 'gemini'))

    expect(await findOnPath(path.join(second, 'gemini'), first)).toBe(path.join(second, 'gemini'))
    expect(await findOnPath(path.join(second, 'absent'), first)).toBeNull()
  })

  it('answers null for a command that is nowhere', async () => {
    const { first, second } = directories()

    expect(await findOnPath('opencode', [first, second].join(path.delimiter))).toBeNull()
    expect(await findOnPath('opencode', '')).toBeNull()
  })
})
