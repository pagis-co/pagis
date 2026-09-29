import * as fs from 'node:fs'
import * as path from 'node:path'

import { loopbackOrigin } from './origin'

/** The port the daemon binds when the config file names no other. */
export const DEFAULT_PORT = 4400

/** The port the Administration Interface binds when the config file
 *  names no other. It is a second listener of the same process,
 *  bound to loopback. */
export const DEFAULT_ADMINISTRATION_PORT = 4401

/**
 * The data directory the daemon boots against. `PAGIS_HOME` wins, as
 * it does in the CLI.
 */
export function dataDirectory(env: NodeJS.ProcessEnv = process.env): string {
  if (env.PAGIS_HOME) {
    return env.PAGIS_HOME
  }
  if (!env.HOME) {
    throw new Error('HOME is not set; set PAGIS_HOME')
  }
  return path.join(env.HOME, '.pagis')
}

export function configPath(home: string): string {
  return path.join(home, 'config.toml')
}

/** The port in the config file, or the default when it names none. */
export function readPort(home: string): number {
  const text = readFile(configPath(home))
  if (text === null) {
    return DEFAULT_PORT
  }
  const line = topLevelLines(text).find((entry) => portKey.test(entry.text))
  return portOf(line?.text, DEFAULT_PORT)
}

/**
 * The Administration Interface's port: the `port` key of the
 * `[administration]` table, or the default when the file names none.
 */
export function readAdministrationPort(home: string): number {
  const text = readFile(configPath(home))
  if (text === null) {
    return DEFAULT_ADMINISTRATION_PORT
  }
  const line = tableLines(text, 'administration').find((entry) =>
    portKey.test(entry),
  )
  return portOf(line, DEFAULT_ADMINISTRATION_PORT)
}

/** The port one `port = ...` line names, or the fallback. */
function portOf(line: string | undefined, fallback: number): number {
  if (line === undefined) {
    return fallback
  }
  const port = Number(line.split('=')[1]?.trim())
  return Number.isInteger(port) && port > 0 && port < 65536 ? port : fallback
}

/**
 * Write the port into the config file. The port is the one key the
 * shell writes (ADR-0025): the daemon owns every other key, so the
 * rest of the file is kept exactly as it stands.
 *
 * The shell can make the State Directory before the daemon starts.
 * Only the OS user reads the directory and the file, as the daemon
 * makes them.
 */
export function writePort(home: string, port: number): void {
  const file = configPath(home)
  const text = readFile(file)
  if (text === null) {
    fs.mkdirSync(home, { recursive: true, mode: 0o700 })
    fs.writeFileSync(file, `port = ${port}\n`, { mode: 0o600 })
    return
  }
  const lines = text.split('\n')
  const existing = topLevelLines(text).find((entry) => portKey.test(entry.text))
  if (existing) {
    lines[existing.index] = `port = ${port}`
  } else {
    lines.unshift(`port = ${port}`)
  }
  fs.writeFileSync(file, lines.join('\n'))
}

/**
 * The Client Credential the daemon wrote, or null before its first boot.
 * The shell exchanges it for a Session; it never goes into a URL.
 */
export function readClientCredential(home: string): string | null {
  return readFile(path.join(home, 'client-credential'))?.trim() ?? null
}

/** The URL the window loads for a server this client started. The
 *  session cookie authenticates it. A server the client did not start
 *  answers at its own origin instead. */
export function daemonUrl(port: number): string {
  return loopbackOrigin(port)
}

/**
 * The URL of the Administration Interface. It is the same host as
 * the product, on the administration port, so the Session cookie of this
 * installation reaches it: a cookie is host-only and no port of that host
 * is a different one to a browser.
 */
export function administrationUrl(port: number): string {
  return loopbackOrigin(port)
}

const portKey = /^\s*port\s*=/

/** The lines of one table of the file, up to the next table header. */
function tableLines(text: string, table: string): string[] {
  const lines: string[] = []
  let inside = false
  for (const line of text.split('\n')) {
    if (/^\s*\[/.test(line)) {
      inside = line.trim() === `[${table}]`
      continue
    }
    if (inside) {
      lines.push(line)
    }
  }
  return lines
}

/**
 * The lines above the first table header. A `port` key inside a table
 * belongs to that table, not to the daemon's own port.
 */
function topLevelLines(text: string): { index: number; text: string }[] {
  const lines: { index: number; text: string }[] = []
  for (const [index, line] of text.split('\n').entries()) {
    if (/^\s*\[/.test(line)) {
      break
    }
    lines.push({ index, text: line })
  }
  return lines
}

function readFile(file: string): string | null {
  try {
    return fs.readFileSync(file, 'utf8')
  } catch {
    return null
  }
}
