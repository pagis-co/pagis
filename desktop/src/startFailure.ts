/** What a failed start uses of the Electron app. */
export interface ExitingApp {
  exit(code?: number): void
}

/** What a failed start uses of the Electron dialog module. */
export interface ErrorBox {
  showErrorBox(title: string, content: string): void
}

/**
 * End the process after the start of the client failed. A Person reads the
 * failure in a native message box, which holds the process until it
 * closes. The smoke test has no Person to close the box, so it prints the
 * failure and exits at once.
 */
export function endFailedStart(error: unknown, smoke: boolean, app: ExitingApp, dialog: ErrorBox): void {
  if (smoke) {
    console.error(`pagis smoke: the client did not start: ${String(error)}`)
  } else {
    dialog.showErrorBox('Pagis could not start', String(error))
  }
  app.exit(1)
}
