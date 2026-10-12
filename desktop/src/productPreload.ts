import { contextBridge, ipcRenderer } from 'electron'

/** The main process gives only this page's sign-in verifier, once. */
if (process.isMainFrame) {
  contextBridge.exposeInMainWorld('pagisDesktop', {
    takeSignInVerifier: (secret: string): Promise<string | undefined> => ipcRenderer.invoke('pagis:sign-in-verifier', secret),
  })
}
