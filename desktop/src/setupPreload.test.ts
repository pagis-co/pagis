// The setup page's bridge to the privileged setup IPC (ADR-0025). Each of
// the three setup paths sends one request, and the validator in the main
// process accepts each one as it is sent.

import { beforeEach, describe, expect, it, vi } from 'vitest'

import { setupRequest } from './setupCoordinator'
import type { SetupApi } from './setupPreload'

const invoke = vi.fn(async (_channel: string, _request: unknown) => undefined)
let exposed: SetupApi | undefined

vi.mock('electron', () => ({
  contextBridge: {
    exposeInMainWorld: (_name: string, api: SetupApi) => { exposed = api },
  },
  ipcRenderer: { invoke, on: vi.fn() },
}))

async function setupApi(): Promise<SetupApi> {
  await import('./setupPreload')
  if (!exposed) throw new Error('the setup preload exposed no API')
  return exposed
}

describe('setup page bridge', () => {
  beforeEach(() => invoke.mockClear())

  it('installs for one Person with a request the main process accepts', async () => {
    const api = await setupApi()

    await api.installHere('one')

    expect(invoke).toHaveBeenCalledWith('pagis:install', { kind: 'local', people: 'one' })
    expect(setupRequest(invoke.mock.calls[0]?.[1])).toEqual({ kind: 'local', people: 'one' })
  })

  it('installs for several People with a request the main process accepts', async () => {
    const api = await setupApi()

    await api.installHere('several')

    expect(invoke).toHaveBeenCalledWith('pagis:install', { kind: 'local', people: 'several' })
    expect(setupRequest(invoke.mock.calls[0]?.[1])).toEqual({ kind: 'local', people: 'several' })
  })

  /** The Person signs in on the server's own page, so the address is
   *  all that the request carries. */
  it('connects to a server with the address alone, in a request the main process accepts', async () => {
    const api = await setupApi()

    await api.connectToServer('pagis.example.com')

    expect(invoke).toHaveBeenCalledWith('pagis:install', { kind: 'server', url: 'pagis.example.com' })
    expect(setupRequest(invoke.mock.calls[0]?.[1])).toEqual({ kind: 'server', url: 'pagis.example.com' })
  })

  /** After a failed Backup, the Person tries the Upgrade again with a
   *  Backup, or continues it without one. */
  it('upgrades again with a Backup or without one', async () => {
    const api = await setupApi()

    await api.upgrade(true)
    await api.upgrade(false)

    expect(invoke.mock.calls).toEqual([['pagis:upgrade', true], ['pagis:upgrade', false]])
  })

  it('tries the connected server again', async () => {
    const api = await setupApi()

    await api.retryServer()

    expect(invoke).toHaveBeenCalledWith('pagis:retry-server')
  })
})
