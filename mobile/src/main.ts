/** The bundled Connect screen of the Mobile App. */

import { CapacitorHttp } from '@capacitor/core'

import { connectToServer } from './connect'
import { mountConnectScreen } from './connectScreen'
import { PagisShell } from './shell'
import './connect.css'

const build = PagisShell.buildType()

mountConnectScreen(document, {
  connect: async (typed) =>
    connectToServer(typed, { ...(await build), request: (options) => CapacitorHttp.request(options) }),
  open: (address) => PagisShell.open(address),
})
