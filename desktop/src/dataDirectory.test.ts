import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import {
  DEFAULT_ADMINISTRATION_PORT,
  DEFAULT_PORT,
  administrationUrl,
  configPath,
  daemonUrl,
  dataDirectory,
  readAdministrationPort,
  readPort,
  readClientCredential,
  writePort,
} from './dataDirectory'

const homes: string[] = []

function newHome(): string {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-home-'))
  homes.push(home)
  return home
}

afterEach(() => {
  while (homes.length > 0) {
    fs.rmSync(homes.pop()!, { recursive: true, force: true })
  }
})

describe('the data directory', () => {
  it('is PAGIS_HOME, or .pagis under the home directory', () => {
    expect(dataDirectory({ PAGIS_HOME: '/data/pagis' })).toBe('/data/pagis')
    expect(dataDirectory({ HOME: '/Users/ada' })).toBe('/Users/ada/.pagis')
    expect(() => dataDirectory({})).toThrow(/HOME/)
  })

  it('reads the default port when the file or the key is absent', () => {
    const home = newHome()
    expect(readPort(home)).toBe(DEFAULT_PORT)
    fs.writeFileSync(configPath(home), 'log_level = "info"\n')
    expect(readPort(home)).toBe(DEFAULT_PORT)
  })

  it('ignores a port key inside a table', () => {
    const home = newHome()
    fs.writeFileSync(configPath(home), '[screen]\nport = 9999\n')
    expect(readPort(home)).toBe(DEFAULT_PORT)
  })

  it('writes the port and keeps every other key', () => {
    const home = newHome()
    fs.writeFileSync(
      configPath(home),
      'port = 4400\nlog_level = "info"\n\n[screen]\nadvertise_ip = "127.0.0.1"\n',
    )

    writePort(home, 4401)

    expect(readPort(home)).toBe(4401)
    const text = fs.readFileSync(configPath(home), 'utf8')
    expect(text).toContain('log_level = "info"')
    expect(text).toContain('advertise_ip = "127.0.0.1"')
    expect(text).not.toContain('port = 4400')
  })

  /** The Client App can make the State Directory before the daemon
   *  starts, so it makes the directory and the file private itself. */
  it('makes a missing State Directory and its config file private', () => {
    const home = path.join(newHome(), 'pagis')
    const previous = process.umask(0o022)
    try {
      writePort(home, 4500)
    } finally {
      process.umask(previous)
    }

    expect(fs.statSync(home).mode & 0o777).toBe(0o700)
    expect(fs.statSync(configPath(home)).mode & 0o777).toBe(0o600)
  })

  it('writes a config file with the port alone when there is none', () => {
    const home = newHome()

    writePort(home, 4500)

    expect(fs.readFileSync(configPath(home), 'utf8')).toBe('port = 4500\n')
  })

  /** The Administration Interface has a port of its own, in a
   *  table of its own, so the daemon's `port` is never read as it. */
  it('reads the administration port out of its own table', () => {
    const home = newHome()
    expect(readAdministrationPort(home)).toBe(DEFAULT_ADMINISTRATION_PORT)

    fs.writeFileSync(configPath(home), 'port = 4400\n\n[administration]\nport = 9443\n')

    expect(readAdministrationPort(home)).toBe(9443)
    expect(readPort(home)).toBe(4400)
    expect(administrationUrl(9443)).toBe('http://127.0.0.1:9443/')
  })

  it('reads the default administration port when the table names none', () => {
    const home = newHome()
    fs.writeFileSync(
      configPath(home),
      'port = 4400\n\n[administration]\nbind = "127.0.0.1"\n\n[screen]\nport = 9999\n',
    )

    expect(readAdministrationPort(home)).toBe(DEFAULT_ADMINISTRATION_PORT)
  })

  it('reads the Client Credential and builds the plain daemon URL', () => {
    const home = newHome()
    expect(readClientCredential(home)).toBeNull()
    fs.writeFileSync(path.join(home, 'client-credential'), `${'a'.repeat(64)}\n`)

    expect(readClientCredential(home)).toBe('a'.repeat(64))
    expect(daemonUrl(4400)).toBe('http://127.0.0.1:4400/')
  })
})
