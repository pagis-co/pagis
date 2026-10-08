import type { MessageBoxOptions, NotificationConstructorOptions } from 'electron'

import { codingSessionsSentence } from './quitQuestion'
import type { CheckResult } from './updates'

/** "Restart to Update" and where it is: in the Pagis menu on macOS, and
 *  on Linux in the tray menu and in the File menu of the window. */
function restartItem(platform: string): string {
  return `Restart to Update in ${platform === 'darwin' ? 'the Pagis menu' : 'the tray menu or in the File menu'}`
}

/** The message box that answers "Check for Updates…". */
export function checkAnswer(result: CheckResult, release: string, platform: string = process.platform): MessageBoxOptions {
  switch (result.kind) {
    case 'up-to-date':
      return { type: 'info', message: 'Pagis is up to date.', detail: `Pagis ${release} is the newest release.` }
    case 'up-to-date-with-server':
      return {
        type: 'info',
        message: 'Pagis is up to date with its server.',
        detail: `This app is Pagis ${release}, and its server runs Pagis ${result.server}. ` +
          'A connected Pagis takes only the Update to the release of its server.',
      }
    case 'found':
      return {
        type: 'info',
        message: `Pagis ${result.version} is available.`,
        detail: `Pagis downloads it now. When the download is complete, select ${restartItem(platform)}.`,
      }
    case 'ready':
      return { type: 'info', message: `Pagis ${result.version} is ready to install.`, detail: `Select ${restartItem(platform)}.` }
    case 'failed':
      return { type: 'error', message: 'Pagis could not check for updates.', detail: result.reason }
  }
}

/**
 * The question that "Restart to Update" asks, or null when it restarts at
 * once (ADR-0027). A restart stops the server, and each Run in progress
 * fails, as at every restart. It also stops the Coding Sessions whose
 * processes this Client App runs (ADR-0033). `unfinished` is null when
 * the server did not give the count.
 */
export function restartQuestion(unfinished: number | null, codingSessions: number): MessageBoxOptions | null {
  const sentences: string[] = []
  if (unfinished === null) {
    sentences.push('Pagis cannot count the Runs in progress. A restart stops each Run that has not finished.')
  } else if (unfinished === 1) {
    sentences.push('1 Run has not finished. A restart stops it.')
  } else if (unfinished > 1) {
    sentences.push(`${unfinished} Runs have not finished. A restart stops them.`)
  }
  if (codingSessions > 0) sentences.push(codingSessionsSentence(codingSessions, 'A restart stops'))
  if (sentences.length === 0) return null
  return {
    type: 'question',
    message: 'Restart Pagis to install the Update?',
    detail: sentences.join(' '),
    buttons: ['Restart', 'Cancel'],
    defaultId: 1,
    cancelId: 1,
  }
}

/** The one notification of an Update that is ready. */
export function readyNotification(version: string, platform: string = process.platform): NotificationConstructorOptions {
  return { title: `Pagis ${version} is ready to install`, body: `Select ${restartItem(platform)}.` }
}
