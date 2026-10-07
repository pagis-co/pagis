// What Home adds to an item of the Needs-You Queue. The daemon derives
// the queue, its order and the line of each item (ADR-0022, ADR-0030).
// Home adds the detail that depends on the reader's locale, the control
// that settles each kind, and the message of the Call back action. The
// module knows nothing about React, so the wording is tested on its
// own.

import type { NeedsYouItem } from '../../api/client'
import { formatMoment } from '../../timeline'
import { failureKindText } from '../runs/runs'

/** One item of the daemon's Needs-You Queue. */
export type QueueItem = NeedsYouItem

/** An inbound call nobody answered. */
export type CallItem = Extract<QueueItem, { kind: 'call' }>

/** Why the item is in the queue, in one line under the daemon's line,
 *  or `null` when there is nothing more. The UI writes it because it
 *  depends on the reader's locale: the clock of the keypad delay and the
 *  words of a failure. A run that waits shows its line and no caption. */
export function queueDetail(item: QueueItem, now: number = Date.now()): string | null {
  switch (item.kind) {
    case 'keypad':
      return now < item.suspended_until
        ? `Pagis checks no keypad code until ${formatMoment(item.suspended_until, now)}. Calls are still answered, as Unknown.`
        : `The delay ended at ${formatMoment(item.suspended_until, now)}. The next wrong code starts a longer delay.`
    case 'call':
      return item.left_message ? 'They left a message.' : 'Nobody answered.'
    case 'failed':
      return failureKindText(item.failure_kind)
    case 'approval':
    case 'waiting':
      return null
  }
}

/** The first message the Call back action writes into the Agent's DM.
 *  It is a message and not a command: the call goes through the Call
 *  Brief and the approval card like every other call (ADR-0020). */
export function callBackDraft(item: CallItem): string {
  const why = item.left_message
    ? 'They called and left a message.'
    : 'They called and nobody answered.'
  return `Please call ${item.remote_e164} back. ${why}`
}

/** Can the reader dismiss the item without its action? A decision, a
 *  question and the keypad delay stay until their own action settles
 *  them; a call nobody answered and a failure only tell the reader. */
export function isDismissible(item: QueueItem): boolean {
  return item.kind === 'call' || item.kind === 'failed'
}

/** The name of the control that settles the item. */
export function queueAction(item: QueueItem): string {
  if (item.kind === 'call') return 'Call back'
  if (item.kind === 'keypad') return 'Clear the count'
  return item.kind === 'failed' ? 'Open run' : 'Open conversation'
}
