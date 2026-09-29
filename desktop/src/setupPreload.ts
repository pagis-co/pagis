import { contextBridge, ipcRenderer } from 'electron'

import type { LocalPeople } from './setupCoordinator'
import type { SetupState } from './setupState'

const api = {
  onState(listener: (state: SetupState) => void): void {
    ipcRenderer.on('pagis:setup-state', (_event, state: SetupState) => listener(state))
    void ipcRenderer.invoke('pagis:setup-state-please')
  },
  /** Install on this computer, for the one Person or for several. */
  installHere: (people: LocalPeople): Promise<void> => ipcRenderer.invoke('pagis:install', { kind: 'local', people }),
  /** Connect to a server this client did not start. The Person signs in
   *  on that server's own page, so the page sends the address alone. */
  connectToServer: (url: string): Promise<void> => ipcRenderer.invoke('pagis:install', { kind: 'server', url }),
  cancel: (): Promise<void> => ipcRenderer.invoke('pagis:cancel-setup'),
  usePort: (port: number): Promise<void> => ipcRenderer.invoke('pagis:setup-use-port', port),
  /** Try again the server that this client is connected to. */
  retryServer: (): Promise<void> => ipcRenderer.invoke('pagis:retry-server'),
  quit: (): Promise<void> => ipcRenderer.invoke('pagis:quit'),
}

export type SetupApi = typeof api
contextBridge.exposeInMainWorld('pagisSetup', api)
