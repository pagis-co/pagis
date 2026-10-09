// What a Coding Session reads as (ADR-0033). The session block in the
// Thread and the head, the transcript and the plan of the session page
// show these words, so they live here and not in each component.
//
// A session shows beside the sprite's face, so its state words are
// neither Activity words nor Computer state words
// (docs/UI-DESIGN.md). The copy calls the program a coding harness, and
// never names the protocol or a tool identifier.

import type { CodingSessionDto, CodingSessionUsage } from '../../api/client'
import type { BadgeTone } from '../../primitives'

/** A state word and the hue of its badge. */
export interface BadgeWord {
  label: string
  tone: BadgeTone
}

const STATE: Record<string, BadgeWord> = {
  starting: { label: 'Opening', tone: 'neutral' },
  working: { label: 'Running', tone: 'working' },
  needs_decision: { label: 'Waits for a decision', tone: 'waiting' },
  idle: { label: 'Ready', tone: 'neutral' },
  interrupted: { label: 'Interrupted', tone: 'waiting' },
  closed: { label: 'Closed', tone: 'neutral' },
  failed: { label: 'Failed', tone: 'failed' },
}

/** The state of a session. A state that this build does not know
 *  shows as the daemon writes it. */
export function sessionStateBadge(state: string): BadgeWord {
  return STATE[state] ?? { label: state, tone: 'neutral' }
}

/** A closed or failed session is settled: it shows its end, and it has
 *  no Stop. */
export function sessionSettled(state: string): boolean {
  return state === 'closed' || state === 'failed'
}

/** Why a session ended. A reason that this build does not know shows as
 *  the daemon writes it. */
export function endReasonText(reason: string | null | undefined, spriteName: string): string {
  switch (reason) {
    case null:
    case undefined:
    case '':
      return 'The session ended'
    case 'closed':
      return `${spriteName} closed the session`
    case 'stopped':
      return 'You stopped the session'
    case 'harness_error':
      return 'The coding harness failed a request'
    case 'harness_exited':
      return 'The coding harness stopped by itself'
    case 'temporarily_unavailable':
      return 'Pagis could not follow the session'
    case 'not_found':
      return 'The coding harness is not installed on the machine'
    case 'bad_directory':
      return 'The directory does not exist'
    case 'worktree_failed':
      return 'Pagis could not make the worktree'
    case 'spawn_failed':
      return 'The coding harness did not start'
    case 'host_not_connected':
      return 'The machine was not connected'
    default:
      return reason
  }
}

/** Where the decision of a `needs_decision` session waits. The block
 *  does not draw the approval card: the card is its own message in the
 *  Thread. */
export function pendingText(
  pending: NonNullable<CodingSessionDto['pending']>,
  spriteName: string,
  harnessName: string,
): string {
  if (pending.kind === 'question') {
    return `${spriteName} answers a question from ${harnessName}.`
  }
  return pending.waits_for === 'person'
    ? 'A permission waits for your answer in this thread.'
    : `${spriteName} decides a permission.`
}

/** Who answers a Harness Permission that Pagis policy does not allow. */
export function approvalModeBadge(mode: string, spriteName: string): BadgeWord {
  switch (mode) {
    case 'person':
      return { label: 'You approve', tone: 'neutral' }
    case 'agent':
      return { label: `${spriteName} approves`, tone: 'neutral' }
    case 'auto':
      return { label: 'Approves everything', tone: 'waiting' }
    default:
      return { label: mode, tone: 'neutral' }
  }
}

/** A cost in its currency. A code that is not ISO 4217 shows as the
 *  harness writes it. */
function costText(amount: number, currency: string): string {
  try {
    return new Intl.NumberFormat(undefined, { style: 'currency', currency }).format(amount)
  } catch {
    return `${amount} ${currency}`
  }
}

/** The share of the context in use and the cost, as far as the harness
 *  reports them. `null` when it reports neither. */
export function usageText(usage: CodingSessionUsage): string | null {
  const parts: string[] = []
  const { context_used: used, context_size: size } = usage
  if (used != null && size != null && size > 0) {
    parts.push(`${Math.round((used / size) * 100)}% of context`)
  }
  if (usage.cost_amount != null && usage.cost_currency != null) {
    parts.push(costText(usage.cost_amount, usage.cost_currency))
  }
  return parts.length === 0 ? null : parts.join(' · ')
}

const TOOL_STATUS: Record<string, BadgeWord> = {
  pending: { label: 'Waiting', tone: 'neutral' },
  in_progress: { label: 'Running', tone: 'working' },
  completed: { label: 'Done', tone: 'neutral' },
  failed: { label: 'Failed', tone: 'failed' },
}

/** The status of a tool call. */
export function toolStatusBadge(status: string): BadgeWord {
  return TOOL_STATUS[status] ?? { label: status, tone: 'neutral' }
}

const TOOL_KIND: Record<string, string> = {
  read: 'Read',
  edit: 'Edit',
  delete: 'Delete',
  move: 'Move',
  search: 'Search',
  execute: 'Command',
  think: 'Think',
  fetch: 'Fetch',
  switch_mode: 'Mode change',
  other: 'Tool',
}

/** The kind of a tool call, in words. A tool call with no kind, or a
 *  kind that this build does not know, is a tool. */
export function toolKindWord(kind: string | null): string {
  return (kind !== null && TOOL_KIND[kind]) || 'Tool'
}

const TURN_END: Record<string, string> = {
  end_turn: 'The turn ended',
  cancelled: 'The turn was stopped',
  max_tokens: 'The turn ran out of tokens',
  max_turn_requests: 'The turn reached its request limit',
  refusal: 'The harness refused the turn',
}

/** Why a turn ended. A reason that this build does not know shows as
 *  the daemon writes it. */
export function turnEndText(stopReason: string): string {
  return TURN_END[stopReason] ?? stopReason
}

/** The end of a Harness Permission: who decided it, and how. */
export function decisionText(
  decision: string,
  decider: string | null,
  spriteName: string,
): string {
  if (decision === 'withdrawn') return 'Cancelled with the turn'
  if (decision === 'cancel') return 'Cancelled'
  const allowed = decision === 'allow_once'
  switch (decider) {
    case 'auto':
      return 'Allowed: the session allows everything'
    case 'scope':
      return "Allowed: inside the session's directory"
    case 'rule':
      return 'Allowed by a rule'
    case 'agent':
      return allowed ? `${spriteName} allowed it` : `${spriteName} denied it`
    case 'person':
      return allowed ? 'You allowed it' : 'You denied it'
    default:
      return allowed ? 'Allowed' : 'Denied'
  }
}

/** Who a Harness Permission or a question waits for. */
export function waitsForText(waitsFor: string | null, spriteName: string): string {
  switch (waitsFor) {
    case 'person':
      return 'Waits for you'
    case 'agent':
      return `Waits for ${spriteName}`
    default:
      return 'Waits for a decision'
  }
}

/** The end of a question of the harness. The answer is `decline`,
 *  `cancel`, `withdrawn`, or the values of the form. */
export function answerText(answer: unknown): string {
  switch (answer) {
    case 'decline':
      return 'Declined'
    case 'cancel':
      return 'Cancelled'
    case 'withdrawn':
      return 'Cancelled with the turn'
    default:
      return 'Answered'
  }
}

const PLAN_STATUS: Record<string, string> = {
  pending: 'Not started',
  in_progress: 'In progress',
  completed: 'Done',
}

/** The status of a plan entry. */
export function planStatusWord(status: string): string {
  return PLAN_STATUS[status] ?? status
}

/** The lines that the changes of a file add and remove. */
export function lineCountText(added: number, removed: number): string {
  return `+${added} −${removed}`
}

/** The count of changes of one file. */
export function changeCountText(changes: number): string {
  return changes === 1 ? '1 change' : `${changes} changes`
}
