import { randomUUID } from 'node:crypto'
import * as path from 'node:path'

import { readExact, writeAtomic } from './clientFiles'
import { serverOrigin } from './origin'

/** The client record of one server. Secrets stay in memory or its partition. */
export interface ServerConnection {
  id: string
  origin: string
  label: string
}

export class ServerConnections {
  private readonly file: string

  constructor(root: string) { this.file = path.join(root, 'servers.json') }

  /** Refuse each shape, origin, label or id that this client cannot write. */
  list(): ServerConnection[] {
    const stored = readExact<{ servers: unknown }>(this.file, ['servers'])
    if (stored === null) return []
    if (!Array.isArray(stored.servers)) throw new Error(`${this.file} is invalid`)
    const ids = new Set<string>()
    const origins = new Set<string>()
    return stored.servers.map((value: unknown) => {
      if (value === null || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${this.file} is invalid`)
      const record = value as Record<string, unknown>
      if (Object.keys(record).sort().join(',') !== 'id,label,origin') throw new Error(`${this.file} has unexpected fields`)
      const { id, origin, label } = record
      if (typeof id !== 'string' || !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id) ||
        typeof origin !== 'string' || serverOrigin(origin) !== origin ||
        typeof label !== 'string' || label !== new URL(origin).host || ids.has(id) || origins.has(origin)) throw new Error(`${this.file} is invalid`)
      ids.add(id); origins.add(origin)
      return { id, origin, label }
    })
  }

  add(address: string): ServerConnection {
    const origin = serverOrigin(address)
    const servers = this.list()
    const held = servers.find((server) => server.origin === origin)
    if (held) return held
    const server = { id: randomUUID(), origin, label: new URL(origin).host }
    writeAtomic(this.file, { servers: [...servers, server] })
    return server
  }

  remove(id: string): void {
    writeAtomic(this.file, { servers: this.list().filter((server) => server.id !== id) })
  }
}
