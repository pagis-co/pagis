import { spawn } from 'node:child_process'
import * as fs from 'node:fs'
import * as path from 'node:path'

import semver from 'semver'

/** The directory of the State Directory that holds the Backup of the last
 *  Upgrade. `pagis backup` leaves it out of each Backup. */
const BACKUPS = 'backups'

/**
 * A Backup that did not complete. The Upgrade stops before the new
 * server opens the data, so the data did not change, and the Person
 * can try again or continue without a Backup.
 */
export class BackupFailure extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'BackupFailure'
  }
}

export interface UpgradeBackupOptions {
  /** The `pagis` program of the old release. */
  program: string
  /** The State Directory, as the daemon gets it in `PAGIS_HOME`. */
  home: string
  /** The old release. It names the Backup. */
  release: string
  signal?: AbortSignal
}

/**
 * Take a Backup with the server program of the old release, and keep it
 * at `<State Directory>/backups/<release>` (ADR-0027).
 *
 * The program writes the Backup into a new directory beside the State
 * Directory, never into it: the program of an older release copies the
 * whole State Directory, and a Backup inside it would copy itself. The
 * same parent puts both on one volume, so the Backup moves into place
 * with one rename. A Backup of the same release that an earlier attempt
 * left goes first, so that such a program does not copy it again.
 *
 * Answers the path of the Backup. Each failure but a cancel is a
 * `BackupFailure`.
 */
export async function takeUpgradeBackup(options: UpgradeBackupOptions): Promise<string> {
  const { program, release, signal } = options
  try {
    // A release has no path separator, so it names one directory.
    if (semver.valid(release) === null) throw new Error(`${JSON.stringify(release)} is not a release`)
    if (!fs.existsSync(program)) throw new BackupFailure(`The server program of Pagis ${release} is not at ${program}.`)
    const home = path.resolve(options.home)
    const backups = path.join(home, BACKUPS)
    const backup = path.join(backups, release)
    if (!present(backups)) fs.mkdirSync(backups, { mode: 0o700 })
    requireDirectory(backups)
    await fs.promises.rm(backup, { recursive: true, force: true })

    const work = fs.mkdtempSync(path.join(path.dirname(home), `${path.basename(home)}-backup-`))
    try {
      const written = path.join(work, 'backup')
      await runBackup(program, home, written, signal)
      fs.renameSync(written, backup)
      return backup
    } finally {
      await fs.promises.rm(work, { recursive: true, force: true })
    }
  } catch (error) {
    if (signal?.aborted || error instanceof BackupFailure) throw error
    const reason = error instanceof Error ? error.message : String(error)
    throw new BackupFailure(`The Backup of Pagis ${release} did not complete: ${reason}`)
  }
}

/**
 * Remove each entry of `<State Directory>/backups` but the Backup of the
 * highest release, so the installation keeps one Backup: the one of its
 * last Upgrade. A Backup is a copy of all the data, so the removal does
 * not hold the main process.
 */
export async function keepNewestBackup(home: string): Promise<void> {
  const backups = path.join(home, BACKUPS)
  if (!present(backups)) return
  requireDirectory(backups)
  const names = fs.readdirSync(backups)
  const newest = names.filter((name) => semver.valid(name) !== null).sort(semver.rcompare)[0]
  for (const name of names) {
    if (name !== newest) await fs.promises.rm(path.join(backups, name), { recursive: true, force: true })
  }
}

/** Run `pagis backup <directory>` on the State Directory, and wait until
 *  the program has ended, also after a cancel. */
function runBackup(program: string, home: string, directory: string, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason)
      return
    }
    const child = spawn(program, ['backup', directory], {
      env: { ...process.env, PAGIS_HOME: home },
      stdio: ['ignore', 'ignore', 'pipe'],
    })
    let output = ''
    let failure: Error | null = null
    child.stderr?.on('data', (chunk: Buffer) => { output += chunk.toString('utf8') })
    child.on('error', (error) => { failure = error })
    const cancel = (): void => { child.kill('SIGTERM') }
    signal?.addEventListener('abort', cancel, { once: true })
    // `close` comes after the program ended, also when it did not start.
    child.on('close', (code, killed) => {
      signal?.removeEventListener('abort', cancel)
      if (signal?.aborted) reject(signal.reason)
      else if (failure) reject(failure)
      else if (code !== 0) reject(new Error(words(output, code, killed)))
      else resolve()
    })
  })
}

/** What the program said, as one line, or how it stopped. */
function words(output: string, code: number | null, signal: NodeJS.Signals | null): string {
  const said = output.trim().replace(/^Error:\s*/, '').replace(/\s+/g, ' ')
  if (said !== '') return said
  return code === null ? `the server program stopped with ${signal ?? 'a signal'}.` : `the server program stopped with code ${code}.`
}

function requireDirectory(directory: string): void {
  const metadata = fs.lstatSync(directory)
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) throw new Error(`${directory} is not a directory of Backups`)
}

/** Whether a file or a link is at `file`, whatever the link names. */
function present(file: string): boolean {
  try {
    fs.lstatSync(file)
    return true
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return false
    throw error
  }
}
