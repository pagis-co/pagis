/** The bundled Connect screen of the Mobile App. */

import { CapacitorHttp } from '@capacitor/core'

import { connectToServer, connectWithScannedLink, type ConnectOptions } from './connect'
import { mountConnectScreen } from './connectScreen'
import { scanQrCode } from './scan'
import { PagisShell } from './shell'
import './connect.css'

const build = PagisShell.buildType()

async function options(): Promise<ConnectOptions> {
  return { ...(await build), request: (request) => CapacitorHttp.request(request) }
}

mountConnectScreen(document, {
  connect: async (typed) => connectToServer(typed, await options()),
  scan: async () => {
    const scanned = await scanQrCode()
    return scanned === null ? null : connectWithScannedLink(scanned, await options())
  },
  open: (address) => PagisShell.open(address),
})
