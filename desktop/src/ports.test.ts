import * as net from 'node:net'

import { afterEach, describe, expect, it } from 'vitest'

import { nextFreePort } from './ports'

const servers: net.Server[] = []

afterEach(async () => {
  while (servers.length > 0) {
    const server = servers.pop()!
    await new Promise<void>((resolve) => server.close(() => resolve()))
  }
})

describe('the next free port', () => {
  it('skips a port that it must not propose, although that port is free', async () => {
    const from = await nextFreePort(15200 + Math.floor(Math.random() * 400))

    const found = await nextFreePort(from, [from])

    expect(found).toBeGreaterThan(from)
  })

  it('skips a port that another process holds', async () => {
    const from = await nextFreePort(15600 + Math.floor(Math.random() * 400))
    const server = net.createServer()
    await new Promise<void>((resolve) => server.listen(from, '127.0.0.1', () => resolve()))
    servers.push(server)

    expect(await nextFreePort(from)).toBeGreaterThan(from)
  })
})
