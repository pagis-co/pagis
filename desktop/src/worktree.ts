// The git worktree of a Coding Session (ADR-0033).
//
// The daemon can ask that a session run in a new git worktree of a
// repository on this machine. The client makes the worktree with the git
// of the person's login shell, so the repository's own hooks run as in the
// person's terminal. The worktree starts at a ref of the local repository:
// the client does not fetch.
//
// The worktrees are in `~/.pagis-worktrees`, apart from the person's
// repositories. They are not in the State Directory `~/.pagis`, because a
// Backup copies that directory. Pagis does not remove a worktree: it holds
// the Agent's work after the session ends, and the person removes it with
// `git worktree remove`.
//
// This module uses only erasable TypeScript syntax and imports only Node
// built-ins and `./loginShell`, so plain `node` loads it in the interop
// test of the daemon.

import { execFile } from 'node:child_process'
import { mkdir, stat } from 'node:fs/promises'
import path from 'node:path'

import { findOnPath } from './loginShell'

/** The most bytes of git's standard error that a failure keeps. */
const MESSAGE_LIMIT = 2 * 1024

/** The worktree that the open request names. */
export interface Worktree {
  /** The repository: an absolute path, or a path under `~/`. */
  repo: string
  /** The new branch of the worktree. */
  branch: string
  /** The ref of the local repository where the branch starts. */
  base: string
}

export interface WorktreeRequest {
  /** The home directory of the OS user. */
  home: string
  /** The directory that the daemon named: the repository or a directory
   *  inside it. */
  cwd: string
  worktree: Worktree
}

/** A `repo` or a `cwd` that is not a directory where a worktree can
 *  start. */
export class BadDirectoryError extends Error {}

/**
 * The path of the worktree of `branch` of the repository `repo`:
 * `<home>/.pagis-worktrees/<base name of repo>/<branch with each "/" as
 * "-">`.
 */
export function worktreePath(home: string, repo: string, branch: string): string {
  return path.join(home, '.pagis-worktrees', path.basename(repo), branch.replaceAll('/', '-'))
}

/**
 * Resolve `~`, and a path that starts with `~/`, against the home
 * directory of the OS user, because the daemon does not know it. Any other
 * path stays as it is.
 */
export function homePath(directory: string, home: string): string {
  if (directory === '~') return home
  return directory.startsWith('~/') ? path.join(home, directory.slice(2)) : directory
}

/**
 * Make the worktree of the request with the git on the `PATH` of `env`,
 * and give the directory where the process runs: the directory in the
 * worktree at the same relative path as `cwd` in the repository.
 *
 * It throws a [`BadDirectoryError`] for a `repo` that is not an absolute
 * path to a directory, and for a `cwd` outside `repo`. Each other failure,
 * such as a git that refuses the branch or the path, throws an error with
 * git's standard error. An abort of `signal` stops git.
 */
export async function makeWorktree(
  request: WorktreeRequest,
  env: Record<string, string>,
  signal: AbortSignal,
): Promise<string> {
  const { home, worktree } = request
  const repo = homePath(worktree.repo, home)
  const cwd = homePath(request.cwd, home)
  await checkRepository(worktree.repo, repo)
  const relative = path.relative(repo, cwd)
  if (relative === '..' || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new BadDirectoryError(`the directory ${request.cwd} is not inside the repository ${worktree.repo}`)
  }

  const git = await findOnPath('git', env.PATH ?? '')
  if (git === null) throw new Error('git is not on the PATH of the login shell')
  const target = worktreePath(home, repo, worktree.branch)
  await mkdir(path.dirname(target), { recursive: true })
  // `--` ends the options, so a path or a base that starts with `-` is not
  // read as an option.
  await run(git, ['-C', repo, 'worktree', 'add', '-b', worktree.branch, '--', target, worktree.base], env, signal)
  return path.join(target, relative)
}

async function checkRepository(named: string, repo: string): Promise<void> {
  if (!path.isAbsolute(repo)) {
    throw new BadDirectoryError(`the repository ${named} is not an absolute path`)
  }
  let isDirectory: boolean
  try {
    isDirectory = (await stat(repo)).isDirectory()
  } catch (error) {
    throw new BadDirectoryError(`the repository ${named} cannot be read: ${(error as Error).message}`)
  }
  if (!isDirectory) throw new BadDirectoryError(`the repository ${named} is not a directory`)
}

/** Run git with no shell. A git that exits with an error rejects with its
 *  standard error. */
function run(file: string, args: string[], env: Record<string, string>, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    execFile(file, args, { env, signal }, (error, _stdout, stderr) => {
      if (error === null) resolve()
      else if (error.name === 'AbortError') reject(error)
      else reject(new Error(gitMessage(stderr) || error.message))
    })
  })
}

/** Git's standard error, trimmed, and cut to its last 2 KiB: git writes
 *  its `fatal:` line after the output of the hooks. */
function gitMessage(stderr: string): string {
  const bytes = Buffer.from(stderr.trim())
  return bytes.subarray(Math.max(0, bytes.length - MESSAGE_LIMIT)).toString('utf8')
}
