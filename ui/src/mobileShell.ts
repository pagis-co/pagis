// The native shell of the Mobile App (`mobile/`), which loads the Product
// App of its server in a web view (ADR-0032). Its plugin `PagisShell`
// answers the main frame of the server alone.

import { Capacitor, registerPlugin } from '@capacitor/core'

interface PagisShellPlugin {
  /** The Session ended. The shell deletes its copy of the Session and
   *  opens its Connect screen. */
  sessionEnded(): Promise<void>
}

const PagisShell = registerPlugin<PagisShellPlugin>('PagisShell')

/** Tell the Mobile App that the Session of this page ended. A browser
 *  has no shell, and nothing happens there. */
export function reportSessionEnded(): void {
  if (!Capacitor.isNativePlatform()) return
  PagisShell.sessionEnded().catch((error: unknown) => {
    console.error('The Mobile App did not take the end of the Session.', error)
  })
}
