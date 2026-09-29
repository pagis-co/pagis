import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { ServerConnections } from './serverConnection'

let root = ''

beforeEach(() => {
  root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-server-connection-'))
})

afterEach(() => {
  fs.rmSync(root, { recursive: true, force: true })
})

describe('the server a connect-only client opens again', () => {
  it('remembers the origin and reads it back', () => {
    const connections = new ServerConnections(root)

    expect(connections.read()).toBeNull()
    expect(connections.write('pagis.example.com')).toEqual({
      origin: 'https://pagis.example.com/',
    })
    expect(connections.read()).toEqual({
      origin: 'https://pagis.example.com/',
    })
  })

  /** A client that installs and supervises a server of its own is not
   *  a client of another server (ADR-0025: nothing converts). */
  it('forgets the server when this client activates a release of its own', () => {
    const connections = new ServerConnections(root)
    connections.write('https://pagis.example.com/')

    connections.forget()

    expect(connections.read()).toBeNull()
    // Forgetting twice is not an error: the local path does it on every
    // activation.
    connections.forget()
  })

  it('keeps no password and no session', () => {
    const connections = new ServerConnections(root)
    connections.write('https://pagis.example.com/')

    const stored = fs.readFileSync(path.join(root, 'server.json'), 'utf8')

    expect(Object.keys(JSON.parse(stored) as object).sort()).toEqual(['origin'])
  })

  /** A stored http:// origin of another computer fails here, and the
   *  start then opens the setup page with the reason. Nothing converts
   *  it. */
  it('refuses to read or write an http:// origin of another computer', () => {
    fs.writeFileSync(path.join(root, 'server.json'), JSON.stringify({
      origin: 'http://192.168.1.10:4400/',
    }))
    expect(() => new ServerConnections(root).read()).toThrow(/only over https:\/\//)

    fs.rmSync(path.join(root, 'server.json'))
    expect(() => new ServerConnections(root).write('http://192.168.1.10:4400'))
      .toThrow(/only over https:\/\//)
    expect(fs.existsSync(path.join(root, 'server.json'))).toBe(false)
  })

  /** An SSH tunnel to a server ends on loopback. */
  it('keeps an http:// origin on loopback', () => {
    const connections = new ServerConnections(root)

    expect(connections.write('http://127.0.0.1:4400')).toEqual({ origin: 'http://127.0.0.1:4400/' })
    expect(connections.read()).toEqual({ origin: 'http://127.0.0.1:4400/' })
  })

  it('refuses a file this client did not write', () => {
    fs.writeFileSync(path.join(root, 'server.json'), JSON.stringify({
      origin: 'file:///etc/passwd',
    }))

    expect(() => new ServerConnections(root).read()).toThrow(/https:\/\/ or http:\/\//)

    fs.writeFileSync(path.join(root, 'server.json'), JSON.stringify({
      origin: 'https://pagis.example.com',
    }))
    expect(() => new ServerConnections(root).read()).toThrow(/is invalid/)

    fs.writeFileSync(path.join(root, 'server.json'), JSON.stringify({
      kind: 'server',
      origin: 'https://pagis.example.com/',
    }))
    expect(() => new ServerConnections(root).read()).toThrow(/unexpected fields/)
  })
})
