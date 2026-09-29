import * as crypto from 'node:crypto'
import * as fs from 'node:fs'
import * as path from 'node:path'

/**
 * How the client keeps its own small records: the active release, the
 * launch marker and the server it connects to.
 *
 * Every one of them is written whole, read with its exact fields, and
 * refused when it is a link or anything but a regular file, so a state
 * file another program wrote cannot steer the client.
 */

export function writeAtomic(file: string, value: unknown): void {
  const root = path.dirname(file)
  requireRoot(root)
  const temporary = `${file}.${crypto.randomUUID()}.tmp`
  fs.writeFileSync(temporary, `${JSON.stringify(value)}\n`, { flag: 'wx', mode: 0o600 })
  fs.renameSync(temporary, file)
}

export function readExact<T>(file: string, fields: string[]): T | null {
  if (!fs.existsSync(file)) return null
  const metadata = fs.lstatSync(file)
  if (!metadata.isFile() || metadata.isSymbolicLink()) throw new Error(`${file} is invalid`)
  const value = JSON.parse(fs.readFileSync(file, 'utf8')) as unknown
  if (value === null || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${file} is invalid`)
  const record = value as Record<string, unknown>
  const actual = Object.keys(record).sort()
  const expected = [...fields].sort()
  if (actual.length !== expected.length || actual.some((field, index) => field !== expected[index])) {
    throw new Error(`${file} has unexpected fields`)
  }
  return record as T
}

export function requireRoot(root: string): void {
  if (!fs.existsSync(root)) fs.mkdirSync(root, { recursive: true, mode: 0o700 })
  const metadata = fs.lstatSync(root)
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) throw new Error(`${root} is not a safe runtime directory`)
}
