/** What the quit uses of the Electron app. */
export interface QuittingApp {
  on(event: 'before-quit', listener: (event: { preventDefault(): void }) => void): unknown
  exit(code?: number): void
}

/**
 * End the process on each quit, after `stop` finished the work of the
 * client: the Host socket, a setup in progress and the server this
 * client started.
 *
 * Electron starts a quit for the Quit Pagis menu item, the Dock, a
 * logout, SIGTERM and SIGINT. `before-quit` holds it while `stop` runs,
 * then `app.exit` ends the process. The client does not call
 * `app.quit()` a second time: after the app held a quit that a signal
 * started, Electron closes the windows on the second `app.quit()` but
 * does not quit, and the process stays with no window. A quit that comes
 * while the client stops joins the first one.
 */
export function endOnQuit(app: QuittingApp, stop: () => Promise<void>): void {
  let ending = false
  app.on('before-quit', (event) => {
    event.preventDefault()
    if (ending) return
    ending = true
    stop().then(
      () => app.exit(0),
      (error: unknown) => {
        console.error(`pagis: the client did not stop cleanly: ${String(error)}`)
        app.exit(1)
      },
    )
  })
}
