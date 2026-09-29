import { execFile } from 'node:child_process'
import * as fs from 'node:fs'
import * as path from 'node:path'

const UNCONFIRMED = 'Pagis could not confirm that the installed server files are unused. Quit other Pagis processes, then select Retry.'

/** True when a process runs `file` or holds it open. */
export function executableInUse(file: string, platform: string = process.platform): Promise<boolean> {
  if (!fs.existsSync(file)) return Promise.resolve(false)
  if (platform === 'linux') return Promise.resolve(procHolds(file))
  return new Promise((resolve, reject) => {
    execFile('/usr/sbin/lsof', ['-t', '--', file], { encoding: 'utf8' }, (error, stdout, stderr) => {
      try { resolve(classifyLsof(error, stdout, stderr)) } catch (failure) { reject(failure) }
    })
  })
}

export function classifyLsof(error: Error | null, stdout: string, stderr: string): boolean {
  if (stderr.trim() !== '') throw new Error(UNCONFIRMED)
  if (!error) return stdout.trim() !== ''
  if ('code' in error && error.code === 1 && stdout.trim() === '' && stderr.trim() === '') return false
  throw new Error(UNCONFIRMED)
}

/**
 * The Linux check reads `/proc`, which every Linux system has, where
 * `lsof` is often not installed. A process uses the file when its
 * executable, an open descriptor or a mapping names it. A process of
 * another account cannot be read, and it cannot run a file under this
 * account's application data either, so it is skipped.
 */
export function procHolds(file: string, proc = '/proc'): boolean {
  const target = path.resolve(file)
  let pids: string[]
  try {
    pids = fs.readdirSync(proc).filter((name) => /^\d+$/.test(name))
  } catch {
    throw new Error(UNCONFIRMED)
  }
  for (const pid of pids) {
    const dir = path.join(proc, pid)
    if (readLink(path.join(dir, 'exe')) === target) return true
    for (const fd of readDir(path.join(dir, 'fd'))) {
      if (readLink(path.join(dir, 'fd', fd)) === target) return true
    }
    const maps = readText(path.join(dir, 'maps'))
    if (maps !== null && maps.split('\n').some((line) => line.endsWith(` ${target}`))) return true
  }
  return false
}

function readLink(link: string): string | null {
  try { return fs.readlinkSync(link) } catch { return null }
}

function readDir(dir: string): string[] {
  try { return fs.readdirSync(dir) } catch { return [] }
}

function readText(file: string): string | null {
  try { return fs.readFileSync(file, 'utf8') } catch { return null }
}
