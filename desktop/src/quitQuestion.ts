import type { MessageBoxOptions } from 'electron'

/**
 * The question that Quit asks, or null when Quit quits at once.
 *
 * Quit asks only while an installation or a start-up of the local
 * server is in progress, because only then does it stop work that the
 * Person waits for. With the setup screens idle, a failure shown, a
 * running server or a connected client, Quit quits at once, as a desktop
 * app does.
 */
export function quitQuestion(inProgress: boolean): MessageBoxOptions | null {
  if (!inProgress) return null
  return {
    type: 'question',
    message: 'Quit Pagis?',
    detail: 'The installation or the start-up in progress stops.',
    buttons: ['Quit', 'Cancel'],
    defaultId: 1,
    cancelId: 1,
  }
}
