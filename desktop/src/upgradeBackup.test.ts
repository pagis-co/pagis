// The Backup that an Upgrade takes with the server program of the old
// release, and the one Backup that the State Directory keeps (ADR-0027).
// A shell script stands in for `pagis backup`, so the tests need no built
// server.

import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { BackupFailure, keepNewestBackup, takeUpgradeBackup } from './upgradeBackup'

const roots: string[] = []
afterEach(() => {
  while (roots.length > 0) fs.rmSync(roots.pop()!, { recursive: true, force: true })
})

/** A parent directory with a State Directory in it, and the program
 *  of the old release beside them. */
function installation(): { parent: string; home: string; program: (script: string) => string } {
  const parent = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-upgrade-backup-'))
  roots.push(parent)
  const home = path.join(parent, '.pagis')
  fs.mkdirSync(path.join(home, 'memory'), { recursive: true })
  fs.writeFileSync(path.join(home, 'runtime-release'), '0.1.0\n')
  fs.writeFileSync(path.join(home, 'memory', 'note.md'), 'remembered')
  const program = (script: string): string => {
    const file = path.join(parent, 'pagis')
    fs.writeFileSync(file, `#!/bin/sh\n${script}\n`, { mode: 0o755 })
    return file
  }
  return { parent, home, program }
}

/** `pagis backup <directory>` of a release with no exclusion for the
 *  Backups: it copies the whole State Directory. It writes what it was
 *  given beside itself. */
const COPY = [
  'printf "%s\\n" "$PAGIS_HOME" "$@" > "$(dirname "$0")/given"',
  '[ "$1" = backup ] || exit 2',
  'mkdir "$2" && cp -R "$PAGIS_HOME" "$2/state" && echo "{}" > "$2/manifest.json"',
].join('\n')

/** The entries of `parent` other than the State Directory and the program. */
function besideHome(parent: string): string[] {
  return fs.readdirSync(parent).filter((name) => !['.pagis', 'pagis', 'given'].includes(name))
}

describe('the Backup of an Upgrade', () => {
  it('runs the old server program on the State Directory and keeps the Backup in it', async () => {
    const { parent, home, program } = installation()

    const backup = await takeUpgradeBackup({ program: program(COPY), home, release: '0.1.0' })

    expect(backup).toBe(path.join(home, 'backups', '0.1.0'))
    expect(fs.readFileSync(path.join(backup, 'state', 'memory', 'note.md'), 'utf8')).toBe('remembered')
    expect(fs.existsSync(path.join(backup, 'manifest.json'))).toBe(true)
    const [given, command, directory] = fs.readFileSync(path.join(parent, 'given'), 'utf8').trim().split('\n')
    expect(given).toBe(home)
    expect(command).toBe('backup')
    // The program writes beside the State Directory and never into it,
    // because the program of an older release copies all of it.
    expect(path.relative(parent, directory).split(path.sep)).toHaveLength(2)
    expect(directory.startsWith(`${home}${path.sep}`)).toBe(false)
    expect(besideHome(parent)).toEqual([])
    expect(fs.statSync(path.join(home, 'backups')).mode & 0o777).toBe(0o700)
  })

  it('replaces the Backup of the same release that an earlier attempt left, and does not copy it', async () => {
    const { home, program } = installation()
    const earlier = path.join(home, 'backups', '0.1.0')
    fs.mkdirSync(path.join(earlier, 'state'), { recursive: true })
    fs.writeFileSync(path.join(earlier, 'state', 'stale'), 'earlier')

    const backup = await takeUpgradeBackup({ program: program(COPY), home, release: '0.1.0' })

    expect(fs.existsSync(path.join(backup, 'state', 'stale'))).toBe(false)
    expect(fs.readdirSync(path.join(backup, 'state', 'backups'))).toEqual([])
  })

  it('stops with the words of the program, and leaves the data and nothing else', async () => {
    const { parent, home, program } = installation()
    const failing = program([
      'mkdir "$2" && echo partial > "$2/partial"',
      `echo "Error: pagis is running against ${home} — stop the daemon first" >&2`,
      'exit 1',
    ].join('\n'))

    const backup = takeUpgradeBackup({ program: failing, home, release: '0.1.0' })

    await expect(backup).rejects.toBeInstanceOf(BackupFailure)
    await expect(backup).rejects.toThrow(
      `The Backup of Pagis 0.1.0 did not complete: pagis is running against ${home} — stop the daemon first`,
    )
    expect(besideHome(parent)).toEqual([])
    expect(fs.existsSync(path.join(home, 'backups', '0.1.0'))).toBe(false)
    expect(fs.readFileSync(path.join(home, 'memory', 'note.md'), 'utf8')).toBe('remembered')
  })

  it('names a missing server program of the old release', async () => {
    const { parent, home } = installation()
    const missing = path.join(parent, 'releases', '0.1.0', 'darwin-arm64', 'pagis')

    const backup = takeUpgradeBackup({ program: missing, home, release: '0.1.0' })

    await expect(backup).rejects.toBeInstanceOf(BackupFailure)
    await expect(backup).rejects.toThrow(`The server program of Pagis 0.1.0 is not at ${missing}.`)
  })

  it('names a program that stops with no words', async () => {
    const { home, program } = installation()

    await expect(takeUpgradeBackup({ program: program('exit 3'), home, release: '0.1.0' }))
      .rejects.toThrow('The Backup of Pagis 0.1.0 did not complete: the server program stopped with code 3.')
  })

  it('stops the program on a cancel and removes what it wrote', async () => {
    const { parent, home, program } = installation()
    const slow = program('mkdir "$2" && echo partial > "$2/partial" && touch "$(dirname "$0")/started" && exec sleep 30')
    const abort = new AbortController()

    const backup = takeUpgradeBackup({ program: slow, home, release: '0.1.0', signal: abort.signal })
    while (!fs.existsSync(path.join(parent, 'started'))) await new Promise((resolve) => setTimeout(resolve, 10))
    abort.abort(new Error('Pagis setup was cancelled'))

    await expect(backup).rejects.toThrow('Pagis setup was cancelled')
    await expect(backup).rejects.not.toBeInstanceOf(BackupFailure)
    expect(besideHome(parent)).toEqual(['started'])
    expect(fs.existsSync(path.join(home, 'backups', '0.1.0'))).toBe(false)
  })

  it('refuses a release name that is not a release', async () => {
    const { home, program } = installation()

    for (const release of ['../0.1.0', '', 'latest']) {
      await expect(takeUpgradeBackup({ program: program(COPY), home, release })).rejects.toThrow(/not a release/)
    }
  })
})

describe('the one Backup that the State Directory keeps', () => {
  it('keeps the Backup of the highest release and removes every other entry', async () => {
    const { home } = installation()
    const backups = path.join(home, 'backups')
    for (const name of ['0.1.0', '0.9.0', '0.10.0', 'copy-of-0.1.0']) fs.mkdirSync(path.join(backups, name), { recursive: true })
    fs.writeFileSync(path.join(backups, 'notes.txt'), 'x')

    await keepNewestBackup(home)

    expect(fs.readdirSync(backups)).toEqual(['0.10.0'])
    expect(fs.readFileSync(path.join(home, 'memory', 'note.md'), 'utf8')).toBe('remembered')
  })

  /** A release can carry build metadata, as the client state accepts. */
  it('takes and keeps the Backup of a release with build metadata', async () => {
    const { home, program } = installation()

    await takeUpgradeBackup({ program: program(COPY), home, release: '1.0.0+signed.1' })
    await keepNewestBackup(home)

    expect(fs.readdirSync(path.join(home, 'backups'))).toEqual(['1.0.0+signed.1'])
  })

  it('does nothing in a State Directory with no Backups', async () => {
    const { home } = installation()

    await keepNewestBackup(home)

    expect(fs.existsSync(path.join(home, 'backups'))).toBe(false)
  })

  it('follows no link out of the State Directory', async () => {
    const { parent, home } = installation()
    const elsewhere = path.join(parent, 'elsewhere')
    fs.mkdirSync(path.join(elsewhere, '0.1.0'), { recursive: true })
    fs.mkdirSync(path.join(elsewhere, '0.2.0'))
    fs.symlinkSync(elsewhere, path.join(home, 'backups'))

    await expect(keepNewestBackup(home)).rejects.toThrow(/not a directory/)
    expect(fs.readdirSync(elsewhere)).toEqual(['0.1.0', '0.2.0'])
  })
})
