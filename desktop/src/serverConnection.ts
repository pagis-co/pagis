import * as fs from 'node:fs'
import * as path from 'node:path'

import { readExact, writeAtomic } from './clientFiles'
import { serverOrigin } from './origin'

/**
 * The server this client connects to, when it did not start one.
 *
 * It holds the origin, which is all a connect-only client needs to open
 * again: the Session lives in the product window's own cookie store and
 * the person's password is never kept. It is client state, like the
 * active release, and it is not a copy of any server-owned onboarding
 * answer (ADR-0025).
 */
export interface ServerConnection {
  origin: string
}

const FILE = 'server.json'

export class ServerConnections {
  constructor(private readonly root: string) {}

  /** The stored connection, or null for a client that starts its own
   *  server. It throws on a file that is not one this client wrote. */
  read(): ServerConnection | null {
    const stored = readExact<ServerConnection>(path.join(this.root, FILE), ['origin'])
    if (stored === null) return null
    if (serverOrigin(stored.origin) !== stored.origin) {
      throw new Error(`${path.join(this.root, FILE)} is invalid`)
    }
    return stored
  }

  /** Remember the server the person signed in to. */
  write(origin: string): ServerConnection {
    const connection: ServerConnection = { origin: serverOrigin(origin) }
    writeAtomic(path.join(this.root, FILE), connection)
    return connection
  }

  /** Forget it. A client that installs and supervises a server of its
   *  own is not a client of another server. */
  forget(): void {
    fs.rmSync(path.join(this.root, FILE), { force: true })
  }
}
