import type { MessageBoxOptions, NotificationConstructorOptions } from 'electron'

import type { CheckResult } from './updates'

const INSTALL = 'Select Restart to Update in the Pagis menu.'

/** The message box that answers "Check for Updates…". */
export function checkAnswer(result: CheckResult, release: string): MessageBoxOptions {
  switch (result.kind) {
    case 'up-to-date':
      return { type: 'info', message: 'Pagis is up to date.', detail: `Pagis ${release} is the newest release.` }
    case 'found':
      return {
        type: 'info',
        message: `Pagis ${result.version} is available.`,
        detail: 'Pagis downloads it now. When the download is complete, select Restart to Update in the Pagis menu.',
      }
    case 'ready':
      return { type: 'info', message: `Pagis ${result.version} is ready to install.`, detail: INSTALL }
    case 'failed':
      return { type: 'error', message: 'Pagis could not check for updates.', detail: result.reason }
  }
}

/**
 * The question that "Restart to Update" asks, or null when it restarts at
 * once (ADR-0027). A restart stops the server, and each Run in progress
 * fails, as at every restart. `unfinished` is null when the server did not
 * give the count.
 */
export function restartQuestion(unfinished: number | null): MessageBoxOptions | null {
  if (unfinished === 0) return null
  const detail = unfinished === null
    ? 'Pagis cannot count the Runs in progress. A restart stops each Run that has not finished.'
    : unfinished === 1
      ? '1 Run has not finished. A restart stops it.'
      : `${unfinished} Runs have not finished. A restart stops them.`
  return {
    type: 'question',
    message: 'Restart Pagis to install the Update?',
    detail,
    buttons: ['Restart', 'Cancel'],
    defaultId: 1,
    cancelId: 1,
  }
}

/** The one notification of an Update that is ready. */
export function readyNotification(version: string): NotificationConstructorOptions {
  return { title: `Pagis ${version} is ready to install`, body: INSTALL }
}
