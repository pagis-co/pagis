/**
 * The Connect screen: one field, "Server address or sign-in link",
 * **Connect**, and **Scan a sign-in link**. A problem shows under the
 * field.
 */

import type { ServerAddress } from './address'

export interface ConnectScreenActions {
  /** Check what the Person typed, and answer the server to open. */
  connect(typed: string): Promise<ServerAddress>
  /** Scan the QR code of a Sign-In Link, check it, and answer the server
   *  to open, or null when the Person closes the scanner. */
  scan(): Promise<ServerAddress | null>
  /** Open the server in the web view. */
  open(address: ServerAddress): Promise<void>
}

export function mountConnectScreen(page: Document, actions: ConnectScreenActions): void {
  const form = page.querySelector<HTMLFormElement>('#connect')!
  const field = page.querySelector<HTMLInputElement>('#address')!
  const problem = page.querySelector<HTMLElement>('#problem')!
  const buttons = [form.querySelector<HTMLButtonElement>('button[type=submit]')!, page.querySelector<HTMLButtonElement>('#scan')!]

  /** Run one try. A problem of the field marks the field and gives it the
   *  focus. */
  function attempt(find: () => Promise<ServerAddress | null>, fromField: boolean): void {
    for (const button of buttons) button.disabled = true
    problem.hidden = true
    problem.textContent = ''
    field.removeAttribute('aria-invalid')
    find()
      .then((address) => (address === null ? undefined : actions.open(address)))
      .catch((error: unknown) => {
        problem.textContent = error instanceof Error ? error.message : String(error)
        problem.hidden = false
        if (fromField) {
          field.setAttribute('aria-invalid', 'true')
          field.focus()
        }
      })
      .finally(() => {
        for (const button of buttons) button.disabled = false
      })
  }

  form.addEventListener('submit', (event) => {
    event.preventDefault()
    attempt(() => actions.connect(field.value), true)
  })
  buttons[1].addEventListener('click', () => attempt(() => actions.scan(), false))
}
