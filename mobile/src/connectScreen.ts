/**
 * The Connect screen: one field, "Server address or sign-in link", and
 * **Connect**. A problem shows under the field.
 */

import type { ServerAddress } from './address'

export interface ConnectScreenActions {
  /** Check what the Person typed, and answer the server to open. */
  connect(typed: string): Promise<ServerAddress>
  /** Open the server in the web view. */
  open(address: ServerAddress): Promise<void>
}

export function mountConnectScreen(page: Document, actions: ConnectScreenActions): void {
  const form = page.querySelector<HTMLFormElement>('#connect')!
  const field = page.querySelector<HTMLInputElement>('#address')!
  const problem = page.querySelector<HTMLElement>('#problem')!
  const button = form.querySelector<HTMLButtonElement>('button[type=submit]')!

  form.addEventListener('submit', (event) => {
    event.preventDefault()
    button.disabled = true
    problem.hidden = true
    problem.textContent = ''
    field.removeAttribute('aria-invalid')
    actions
      .connect(field.value)
      .then((address) => actions.open(address))
      .catch((error: unknown) => {
        problem.textContent = error instanceof Error ? error.message : String(error)
        problem.hidden = false
        field.setAttribute('aria-invalid', 'true')
        field.focus()
      })
      .finally(() => {
        button.disabled = false
      })
  })
}
