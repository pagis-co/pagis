/**
 * The Unreachable screen. The shell opens it in place of the stored
 * server when the web view cannot load the server: the name does not
 * resolve, the server does not answer, or TLS fails. The query of the
 * page holds `server`, the origin of the server, and `error`, the text of
 * the error. The screen shows both, **Try again** and **Change server**.
 */

import type { ServerAddress } from './address'

export interface UnreachableScreenActions {
  /** Open the server in the web view again. */
  open(address: ServerAddress): Promise<void>
  /** Forget the server and the copy of the Session, and show the Connect
   *  screen. */
  changeServer(): Promise<void>
}

export function mountUnreachableScreen(page: Document, query: string, actions: UnreachableScreenActions): void {
  const params = new URLSearchParams(query)
  const server = params.get('server') ?? ''
  page.querySelector<HTMLElement>('#server')!.textContent = server
  page.querySelector<HTMLElement>('#error')!.textContent = params.get('error') ?? ''

  const problem = page.querySelector<HTMLElement>('#problem')!
  const retry = page.querySelector<HTMLButtonElement>('#retry')!
  const change = page.querySelector<HTMLButtonElement>('#change')!
  const buttons = [retry, change]

  function attempt(act: () => Promise<void>): void {
    for (const button of buttons) button.disabled = true
    problem.hidden = true
    problem.textContent = ''
    act()
      .catch((error: unknown) => {
        problem.textContent = error instanceof Error ? error.message : String(error)
        problem.hidden = false
      })
      .finally(() => {
        for (const button of buttons) button.disabled = false
      })
  }

  retry.addEventListener('click', () => attempt(() => actions.open({ origin: server, opens: server })))
  change.addEventListener('click', () => attempt(() => actions.changeServer()))
}
