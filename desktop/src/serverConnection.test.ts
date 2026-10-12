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

describe('the servers a connected Client App opens again', () => {
  it('remembers the origin and reads it back', () => {
    const connections = new ServerConnections(root)

    expect(connections.list()).toEqual([])
    expect(connections.add('pagis.example.com')).toEqual(expect.objectContaining({ id: expect.any(String), origin: 'https://pagis.example.com/', label: 'pagis.example.com' }))
    expect(connections.list()).toEqual([expect.objectContaining({ id: expect.any(String), origin: 'https://pagis.example.com/', label: 'pagis.example.com' })])
  })

  it('removes a server and permits a second removal', () => {
    const connections = new ServerConnections(root)
    const { id } = connections.add('https://pagis.example.com/')

    connections.remove(id)

    expect(connections.list()).toEqual([])
    connections.remove(id)
  })

  it('keeps no password and no session', () => {
    const connections = new ServerConnections(root)
    connections.add('https://pagis.example.com/')

    const stored = fs.readFileSync(path.join(root, 'servers.json'), 'utf8')

    expect(Object.keys(JSON.parse(stored).servers[0]).sort()).toEqual(['id', 'label', 'origin'])
  })

  /** A Sign-In Link carries its secret after `#`. The client keeps the
   *  origin of the link, and a stored link is not a file it wrote. */
  it('keeps the origin of a Sign-In Link and never its secret', () => {
    const secret = 'not-a-real-link-secret'
    const connections = new ServerConnections(root)

    expect(connections.add(`https://pagis.example.com/sign-in#${secret}`)).toEqual(expect.objectContaining({ id: expect.any(String), origin: 'https://pagis.example.com/', label: 'pagis.example.com' }))

    expect(fs.readFileSync(path.join(root, 'servers.json'), 'utf8')).not.toContain(secret)
    expect(fs.readdirSync(root)).toEqual(['servers.json'])

    fs.writeFileSync(path.join(root, 'servers.json'), JSON.stringify({ servers: [{ id: 'aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa', label: 'pagis.example.com',
      origin: `https://pagis.example.com/sign-in#${secret}`,
    }] }))
    expect(() => new ServerConnections(root).list()).toThrow(/is invalid/)
  })

  /** A stored http:// origin of another computer fails here, and the
   *  start then opens the setup page with the reason. Nothing converts
   *  it. */
  it('refuses to read or write an http:// origin of another computer', () => {
    fs.writeFileSync(path.join(root, 'servers.json'), JSON.stringify({ servers: [{ id: 'aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa', label: 'pagis.example.com',
      origin: 'http://192.168.1.10:4400/',
    }] }))
    expect(() => new ServerConnections(root).list()).toThrow(/only over https:\/\//)

    fs.rmSync(path.join(root, 'servers.json'))
    expect(() => new ServerConnections(root).add('http://192.168.1.10:4400'))
      .toThrow(/only over https:\/\//)
    expect(fs.existsSync(path.join(root, 'servers.json'))).toBe(false)
  })

  /** An SSH tunnel to a server ends on loopback. */
  it('keeps an http:// origin on loopback', () => {
    const connections = new ServerConnections(root)

    expect(connections.add('http://127.0.0.1:4400')).toEqual(expect.objectContaining({ origin: 'http://127.0.0.1:4400/', label: '127.0.0.1:4400' }))
    expect(connections.list()).toEqual([expect.objectContaining({ origin: 'http://127.0.0.1:4400/', label: '127.0.0.1:4400' })])
  })

  it('refuses a file this client did not write', () => {
    fs.writeFileSync(path.join(root, 'servers.json'), JSON.stringify({ servers: [{ id: 'aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa', label: 'pagis.example.com',
      origin: 'file:///etc/passwd',
    }] }))

    expect(() => new ServerConnections(root).list()).toThrow(/https:\/\/ or http:\/\//)

    fs.writeFileSync(path.join(root, 'servers.json'), JSON.stringify({ servers: [{ id: 'aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa', label: 'pagis.example.com',
      origin: 'https://pagis.example.com',
    }] }))
    expect(() => new ServerConnections(root).list()).toThrow(/is invalid/)

    fs.writeFileSync(path.join(root, 'servers.json'), JSON.stringify({ servers: [{ id: 'aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa', label: 'pagis.example.com',
      kind: 'server',
      origin: 'https://pagis.example.com/',
    }] }))
    expect(() => new ServerConnections(root).list()).toThrow(/unexpected fields/)
  })
})

it('keeps several servers in order, removes only one and uses stable ids', () => {
  const connections = new ServerConnections(root)
  const home = connections.add('home.example.com')
  const work = connections.add('work.example.com:8443')
  expect(work.label).toBe('work.example.com:8443')
  expect(work.id).not.toBe(home.id)
  expect(new ServerConnections(root).list()).toEqual([home, work])
  expect(connections.add(home.origin)).toEqual(home)
  expect(connections.list()).toEqual([home, work])
  connections.remove(home.id)
  expect(new ServerConnections(root).list()).toEqual([work])
})

it.each([
  { servers: 'bad' },
  { servers: [null] },
  { servers: [{ id: '../bad', origin: 'https://a.example/', label: 'a.example' }] },
  { servers: [{ id: 'aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa', origin: 'https://a.example/', label: 'Wrong' }] },
])('refuses malformed server lists: %j', (value) => {
  fs.writeFileSync(path.join(root, 'servers.json'), JSON.stringify(value))
  expect(() => new ServerConnections(root).list()).toThrow(/invalid/)
})

it('refuses duplicate ids, duplicate origins and a symbolic link', () => {
  const connections = new ServerConnections(root)
  const server = connections.add('a.example')
  const file = path.join(root, 'servers.json')
  for (const other of [server, { ...server, origin: 'https://b.example/', label: 'b.example' }, { ...server, id: 'bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb' }]) {
    fs.writeFileSync(file, JSON.stringify({ servers: [server, other] }))
    expect(() => connections.list()).toThrow(/invalid/)
  }
  fs.renameSync(file, path.join(root, 'elsewhere.json'))
  fs.symlinkSync(path.join(root, 'elsewhere.json'), file)
  expect(() => connections.list()).toThrow(/invalid/)
})
