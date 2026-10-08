// The git worktree of a Coding Session. The cases make a real repository
// with one commit in a temporary directory, and give a temporary home
// directory, so each worktree goes under a home of the test. They serve a
// real session stream with `startSession`, so they see the answer of the
// stream and the directory where the process runs.

import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { duplexPair } from 'node:stream'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { findOnPath } from './loginShell'
import { type SessionDeps, type SessionExitFrame, startSession } from './sessions'
import { worktreePath } from './worktree'

const GIT_ENVIRONMENT = {
  // The git configuration of the person who runs the tests does not apply.
  GIT_CONFIG_GLOBAL: '/dev/null',
  GIT_CONFIG_NOSYSTEM: '1',
}

const pids: number[] = []

afterEach(() => {
  for (const pid of pids.splice(0)) {
    if (isAlive(pid)) process.kill(pid, 'SIGKILL')
  }
})

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

function directory(): string {
  return realpathSync(mkdtempSync(path.join(os.tmpdir(), 'pagis-worktree-')))
}

function git(repo: string, ...args: string[]): string {
  return execFileSync('git', ['-C', repo, ...args], {
    env: { ...process.env, ...GIT_ENVIRONMENT },
    encoding: 'utf8',
  }).trim()
}

/** A repository `shop` with one commit on `main`, which holds the
 *  directory `web`. */
function repository(): { repo: string; base: string } {
  const repo = path.join(directory(), 'shop')
  mkdirSync(path.join(repo, 'web'), { recursive: true })
  writeFileSync(path.join(repo, 'web', 'index.html'), '<p>shop</p>\n')
  git(repo, 'init', '--quiet', '--initial-branch=main')
  git(repo, 'add', '.')
  git(repo, '-c', 'user.name=Test', '-c', 'user.email=test@example.com', 'commit', '--quiet', '--message=start')
  return { repo, base: git(repo, 'rev-parse', 'HEAD') }
}

/** The login-shell environment: a PATH with git and node. */
async function shellEnvironment(): Promise<Record<string, string>> {
  const found = await findOnPath('git', process.env.PATH ?? '')
  if (found === null) throw new Error('the tests need git on the PATH')
  return {
    PATH: [path.dirname(found), path.dirname(process.execPath)].join(path.delimiter),
    ...GIT_ENVIRONMENT,
  }
}

/** A request whose process prints the directory where it runs. */
function request(cwd: string, worktree: { repo: string; branch: string; base: string }, command = process.execPath): string {
  return `${JSON.stringify({
    session_id: 'session-1',
    command,
    args: ['-e', 'process.stdout.write(process.cwd())'],
    cwd,
    env: {},
    worktree,
  })}\n`
}

/** A session stream with the home directory `home`. The test is the
 *  daemon at the other end. */
function session(home: string, deps: Partial<SessionDeps> = {}) {
  const [client, daemon] = duplexPair()
  const chunks: Buffer[] = []
  daemon.on('data', (chunk: Buffer) => chunks.push(chunk))
  daemon.on('error', () => {})
  const exits: SessionExitFrame[] = []
  const onSpawn = vi.fn()
  const done = startSession(client, {
    environment: shellEnvironment,
    home,
    send: (frame) => exits.push(frame),
    graceMs: 200,
    onSpawn,
    ...deps,
  })
  const output = (): string => Buffer.concat(chunks).toString()
  return {
    client,
    daemon,
    exits,
    onSpawn,
    done,
    output,
    answer: async (): Promise<Record<string, unknown>> => {
      await vi.waitFor(() => expect(output()).toContain('\n'), { timeout: 5_000 })
      return JSON.parse(output().slice(0, output().indexOf('\n'))) as Record<string, unknown>
    },
    stdout: (): string => output().slice(output().indexOf('\n') + 1),
  }
}

describe('the path of a worktree', () => {
  it('is the base name of the repository and the branch with each / as -, under ~/.pagis-worktrees', () => {
    expect(worktreePath('/Users/ana', '/Users/ana/code/shop', 'pagis/fix-login')).toBe(
      '/Users/ana/.pagis-worktrees/shop/pagis-fix-login',
    )
    expect(worktreePath('/Users/ana', '/Users/ana/code/shop/', 'fix-login')).toBe(
      '/Users/ana/.pagis-worktrees/shop/fix-login',
    )
  })
})

describe('a session with a worktree', () => {
  it('makes the branch from base at the worktree path, and runs the process at the same relative directory', async () => {
    const home = directory()
    const { repo, base } = repository()
    // The base moves on after the request names it: the branch starts at
    // the named commit.
    writeFileSync(path.join(repo, 'later'), '')
    git(repo, 'add', '.')
    git(repo, '-c', 'user.name=Test', '-c', 'user.email=test@example.com', 'commit', '--quiet', '--message=later')
    const { daemon, done, answer, stdout } = session(home)
    const worktree = path.join(home, '.pagis-worktrees', 'shop', 'pagis-fix-login')

    daemon.write(request(path.join(repo, 'web'), { repo, branch: 'pagis/fix-login', base }))

    expect(await answer()).toEqual({ ok: true, cwd: path.join(worktree, 'web') })
    await done
    expect(stdout()).toBe(path.join(worktree, 'web'))
    expect(git(worktree, 'rev-parse', '--abbrev-ref', 'HEAD')).toBe('pagis/fix-login')
    expect(git(worktree, 'rev-parse', 'HEAD')).toBe(base)
    expect(readFileSync(path.join(worktree, 'web', 'index.html'), 'utf8')).toBe('<p>shop</p>\n')
  })

  it('resolves a repo under ~/ against the home directory of the OS user', async () => {
    const { repo, base } = repository()
    const home = path.dirname(repo)
    const { daemon, answer } = session(home)

    daemon.write(request('~/shop', { repo: '~/shop', branch: 'pagis/fix', base }))

    expect(await answer()).toEqual({ ok: true, cwd: path.join(home, '.pagis-worktrees', 'shop', 'pagis-fix') })
  })

  it('answers worktree_failed with the message of git for a second request with the same branch', async () => {
    const home = directory()
    const { repo, base } = repository()
    const first = session(home)
    first.daemon.write(request(repo, { repo, branch: 'pagis/fix-login', base }))
    expect(await first.answer()).toMatchObject({ ok: true })
    await first.done

    const second = session(home)
    second.daemon.write(request(repo, { repo, branch: 'pagis/fix-login', base }))

    expect(await second.answer()).toEqual({
      ok: false,
      error: 'worktree_failed',
      message: expect.stringMatching(/\nfatal: a branch named 'pagis\/fix-login' already exists$/),
    })
    await second.done
    expect(second.onSpawn).not.toHaveBeenCalled()
  })

  it('answers worktree_failed with the message of git for a repo that is not a git repository', async () => {
    const home = directory()
    const repo = directory()
    const { daemon, answer, done, onSpawn } = session(home)

    daemon.write(request(repo, { repo, branch: 'pagis/fix', base: 'main' }))

    expect(await answer()).toEqual({
      ok: false,
      error: 'worktree_failed',
      message: expect.stringMatching(/^fatal: not a git repository/),
    })
    await done
    expect(onSpawn).not.toHaveBeenCalled()
  })

  it('answers bad_directory for a repo that does not exist, a relative repo and a cwd outside the repo', async () => {
    const home = directory()
    const { repo, base } = repository()
    const missing = path.join(path.dirname(repo), 'missing')
    // A sibling whose name starts with the name of the repository.
    const sibling = `${repo}-web`
    mkdirSync(sibling)
    const cases = [
      { cwd: repo, worktree: { repo: missing, branch: 'pagis/fix', base } },
      { cwd: repo, worktree: { repo: 'code/shop', branch: 'pagis/fix', base } },
      { cwd: home, worktree: { repo, branch: 'pagis/fix', base } },
      { cwd: sibling, worktree: { repo, branch: 'pagis/fix', base } },
    ]

    for (const { cwd, worktree } of cases) {
      const { daemon, answer, done } = session(home)
      daemon.write(request(cwd, worktree))
      expect(await answer()).toEqual({ ok: false, error: 'bad_directory', message: expect.any(String) })
      await done
    }
    expect(existsSync(path.join(home, '.pagis-worktrees'))).toBe(false)
  })

  it('answers worktree_failed when git is not on the PATH of the login shell', async () => {
    const home = directory()
    const { repo, base } = repository()
    const { daemon, answer, done, onSpawn } = session(home, {
      environment: async () => ({ PATH: directory() }),
    })

    daemon.write(request(repo, { repo, branch: 'pagis/fix', base }))

    expect(await answer()).toEqual({
      ok: false,
      error: 'worktree_failed',
      message: 'git is not on the PATH of the login shell',
    })
    await done
    expect(onSpawn).not.toHaveBeenCalled()
  })

  it('stops git when the stream closes while git runs, and starts no process', async () => {
    const home = directory()
    const { repo, base } = repository()
    // A git that writes its pid and then runs until a signal ends it.
    const bin = directory()
    const pidFile = path.join(bin, 'pid')
    writeFileSync(path.join(bin, 'git'), `#!/bin/sh\necho $$ > '${pidFile}'\nexec /bin/sleep 30\n`, { mode: 0o755 })
    const { client, daemon, done, onSpawn, exits, output } = session(home, {
      environment: async () => ({ PATH: bin }),
    })

    daemon.write(request(repo, { repo, branch: 'pagis/fix', base }))
    await vi.waitFor(() => expect(readFileSync(pidFile, 'utf8')).toMatch(/^\d+\n$/), { timeout: 5_000 })
    const pid = Number(readFileSync(pidFile, 'utf8'))
    pids.push(pid)

    // A reset of the stream destroys the client's side, as yamux does.
    client.destroy()

    await done
    await vi.waitFor(() => expect(isAlive(pid)).toBe(false))
    expect(onSpawn).not.toHaveBeenCalled()
    expect(exits).toEqual([])
    expect(output()).toBe('')
  })
})
