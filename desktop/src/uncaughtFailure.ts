import type { EventEmitter } from 'node:events'

import { installationError } from './runtimeInstaller'

/**
 * Give each uncaught exception of the main process to `show`, which puts
 * it on the setup page. Electron shows the raw JavaScript error in a
 * native dialog only when the process has no listener for the event, so
 * this listener also keeps that dialog away. The error that `show` gets
 * holds no Client Credential, Session or sign-in link.
 */
export function reportUncaughtExceptions(target: EventEmitter, show: (error: Error) => void): void {
  target.on('uncaughtException', (error: unknown) => {
    const failure = installationError(error)
    console.error(`pagis: uncaught error: ${failure.message}`)
    show(failure)
  })
}
