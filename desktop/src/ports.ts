import * as net from 'node:net'

import { LOOPBACK_HOST } from './origin'

/**
 * The next free port at or after `from` on the address the server binds.
 * The taken-port page proposes it, so the user always knows the port:
 * the shell never picks a random one (ADR-0025). Only a server this
 * client starts needs one, and that server binds loopback.
 *
 * `reserved` holds the ports that the server binds for another listener,
 * such as the Administration Port. Such a port can be free only because
 * the server does not run, so the search skips it.
 */
export async function nextFreePort(
  from: number,
  reserved: readonly number[] = [],
  tries = 50,
  host = LOOPBACK_HOST,
): Promise<number> {
  for (let port = from; port < from + tries && port < 65536; port += 1) {
    if (!reserved.includes(port) && await isFree(port, host)) {
      return port
    }
  }
  throw new Error(`no free port between ${from} and ${from + tries}`)
}

export function isFree(port: number, host = LOOPBACK_HOST): Promise<boolean> {
  return new Promise((resolve) => {
    const server = net.createServer()
    server.once('error', () => resolve(false))
    server.listen(port, host, () => {
      server.close(() => resolve(true))
    })
  })
}
