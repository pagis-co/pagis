import type { MessageBoxOptions } from 'electron'

/**
 * The sentence that names the Coding Sessions on this computer that a
 * quit or a restart stops. `stops` is the start of its second part, such
 * as "Quitting stops".
 */
export function codingSessionsSentence(count: number, stops: string): string {
  return count === 1
    ? `1 Coding Session runs on this computer. ${stops} it.`
    : `${count} Coding Sessions run on this computer. ${stops} them.`
}

/**
 * The question that Quit asks, or null when Quit quits at once.
 *
 * Quit asks only while it stops work: an installation or a start-up of
 * the local server in progress, or the Coding Sessions whose processes
 * this Client App runs (ADR-0033). The detail names each kind of work
 * that stops. With nothing in progress and no Coding Session, Quit quits
 * at once, as a desktop app does.
 */
export function quitQuestion(inProgress: boolean, codingSessions: number): MessageBoxOptions | null {
  const sentences: string[] = []
  if (inProgress) sentences.push('The installation or the start-up in progress stops.')
  if (codingSessions > 0) sentences.push(codingSessionsSentence(codingSessions, 'Quitting stops'))
  if (sentences.length === 0) return null
  return {
    type: 'question',
    message: 'Quit Pagis?',
    detail: sentences.join(' '),
    buttons: ['Quit', 'Cancel'],
    defaultId: 1,
    cancelId: 1,
  }
}
