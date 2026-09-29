import { contextBridge, ipcRenderer } from 'electron'

import type { DaemonState } from './daemon'

/**
 * The bridge for the shell's own status page. The daemon's SPA has no
 * preload, so nothing of this reaches the product UI.
 */
const api = {
  onState(listener: (state: DaemonState) => void): void {
    ipcRenderer.on('pagis:state', (_event, state: DaemonState) => listener(state))
    void ipcRenderer.invoke('pagis:state-please')
  },
  retry: (): Promise<void> => ipcRenderer.invoke('pagis:retry'),
  usePort: (port: number): Promise<void> => ipcRenderer.invoke('pagis:use-port', port),
  revealLogs: (): Promise<void> => ipcRenderer.invoke('pagis:reveal-logs'),
  quit: (): Promise<void> => ipcRenderer.invoke('pagis:quit'),
}

export type StatusApi = typeof api

contextBridge.exposeInMainWorld('pagisStatus', api)
