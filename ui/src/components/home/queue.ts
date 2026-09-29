// The rules of the Needs-You Queue: which work
// waits for the reader, in which order, and what each line says. The
// module knows nothing about React, so the wording is tested on its
// own.

import type { CallSummaryDto, KeypadCodeDto, RequestDto, RunDto } from '../../api/client'
import type { LiveRun } from '../../state/presence'
import { dayKey, formatMoment } from '../../timeline'
import { failureText } from '../runs/runs'

/** The kinds of work that wait for the reader, most urgent first. */
export const QUEUE_KINDS = ['approval', 'waiting', 'keypad', 'call', 'failed'] as const
export type QueueKind = (typeof QUEUE_KINDS)[number]

/** A pending decision: the card carries the Approve and the Deny. */
export interface ApprovalItem {
  kind: 'approval'
  id: string
  agentId: string
  /** The Request row the card reads its state from. */
  requestId: string
  title: string
  body: string
  at: number
}

/** A run that waits for an answer, or one that failed. */
export interface RunItem {
  kind: 'waiting' | 'failed'
  id: string
  agentId: string
  channelId: string | null
  runId: string
  /** Why it is here, in one line, or null when there is nothing more. */
  detail: string | null
  at: number
}

/** An inbound call nobody answered. The reader answers it by asking
 *  the Agent to call the number back. */
export interface CallItem {
  kind: 'call'
  id: string
  agentId: string
  /** The Remote Party, in E.164. */
  remoteE164: string
  /** Did the caller leave a message? */
  leftMessage: boolean
  detail: string | null
  at: number
}

/** Callers entered so many wrong keypad codes that a delay started
 *  (ADR-0021). The notice belongs to the Workspace and to no Agent,
 *  and it stays until a correct code or the reader clears the count. */
export interface KeypadItem {
  kind: 'keypad'
  id: 'keypad'
  failedAttempts: number
  /** The end of the latest delay. */
  suspendedUntil: number
  detail: string
  at: number
}

export type QueueItem = ApprovalItem | RunItem | CallItem | KeypadItem

interface ApprovalPayload {
  action_title?: string
  tool_name?: string
  body?: string
  domain?: string
}

/** The queue line of one pending Request. */
export function approvalItem(request: RequestDto): ApprovalItem {
  const payload = (request.payload ?? {}) as ApprovalPayload
  return {
    kind: 'approval',
    id: request.id,
    agentId: request.agent_id,
    requestId: request.id,
    title: payload.action_title ?? payload.tool_name ?? payload.domain ?? 'An action',
    body: payload.body ?? payload.tool_name ?? '',
    at: request.created_at,
  }
}

/** The outcomes an inbound call reaches when nobody spoke to the
 *  caller (ADR-0020). `answered` is the only outcome that says a
 *  person or an Agent took the call. */
const MISSED_OUTCOMES = ['no_answer', 'busy', 'voicemail', 'failed']

/** The queue line of one missed inbound call. */
export function missedCallItem(call: CallSummaryDto): CallItem {
  const leftMessage = call.outcome === 'voicemail'
  return {
    kind: 'call',
    id: call.id,
    agentId: call.agent_id,
    remoteE164: call.remote_e164,
    leftMessage,
    detail: leftMessage ? 'They left a message.' : 'Nobody answered.',
    at: call.ended_at ?? call.created_at,
  }
}

/** The keypad notice, once a delay has started; `null` before the
 *  first delay and after the count is cleared. */
export function keypadItem(keypad: KeypadCodeDto, now: number): KeypadItem | null {
  const until = keypad.suspended_until
  if (until === null || until === undefined) return null
  const detail =
    now < until
      ? `Pagis checks no keypad code until ${formatMoment(until, now)}. Calls are still answered, as Unknown.`
      : `The delay ended at ${formatMoment(until, now)}. The next wrong code starts a longer delay.`
  return {
    kind: 'keypad',
    id: 'keypad',
    failedAttempts: keypad.failed_attempts,
    suspendedUntil: until,
    detail,
    at: until,
  }
}

/** Is it an inbound call of today that nobody answered? */
export function isMissedCall(call: CallSummaryDto, now: number): boolean {
  return (
    call.direction === 'inbound' &&
    call.state === 'ended' &&
    call.outcome !== null &&
    call.outcome !== undefined &&
    MISSED_OUTCOMES.includes(call.outcome) &&
    isToday(call.ended_at ?? call.created_at, now)
  )
}

/** The first message the Call back action writes into the Agent's DM.
 *  It is a message and not a command: the call goes through the Call
 *  Brief and the approval card like every other call (ADR-0020). */
export function callBackDraft(item: CallItem): string {
  const why = item.leftMessage
    ? 'They called and left a message.'
    : 'They called and nobody answered.'
  return `Please call ${item.remoteE164} back. ${why}`
}

/** The queue line of one failed run. */
export function failedItem(run: RunDto): RunItem {
  return {
    kind: 'failed',
    id: run.id,
    agentId: run.agent_id,
    channelId: run.channel_id ?? null,
    runId: run.id,
    detail: failureText(run),
    at: run.ended_at ?? run.created_at,
  }
}

/** The queue line of one run that waits for the reader. */
export function waitingItem(run: LiveRun): RunItem {
  return {
    kind: 'waiting',
    id: run.runId,
    agentId: run.agentId,
    channelId: run.channelId,
    runId: run.runId,
    detail: run.caption,
    at: 0,
  }
}

/** Is the run one the reader must answer? */
export function isWaitingForUser(run: LiveRun): boolean {
  return run.state === 'waiting_for_user'
}

/** Did it happen on this calendar day? */
export function isToday(at: number, now: number = Date.now()): boolean {
  return dayKey(at) === dayKey(now)
}

/**
 * The Needs you queue.
 *
 * A decision comes first, then a question, then a keypad delay, then a
 * call nobody answered, then a failure; inside one kind the newest is
 * first. Only today's calls and failures join, so the queue stays a
 * queue and not a record.
 */
export function buildQueue({
  requests,
  liveRuns,
  failedRuns,
  calls,
  keypad = null,
  now = Date.now(),
}: {
  requests: readonly RequestDto[]
  liveRuns: readonly LiveRun[]
  failedRuns: readonly RunDto[]
  calls: readonly CallSummaryDto[]
  /** The Keypad Code card of the Workspace, when it has landed. */
  keypad?: KeypadCodeDto | null
  now?: number
}): QueueItem[] {
  const notice = keypad === null ? null : keypadItem(keypad, now)
  const items: QueueItem[] = [
    ...requests.filter((request) => request.state === 'pending').map(approvalItem),
    ...liveRuns.filter(isWaitingForUser).map(waitingItem),
    ...(notice === null ? [] : [notice]),
    ...calls.filter((call) => isMissedCall(call, now)).map(missedCallItem),
    ...failedRuns
      .filter(
        (run) =>
          run.state === 'failed' && isToday(run.ended_at ?? run.created_at, now),
      )
      .map(failedItem),
  ]
  return items.sort((left, right) => {
    const byKind = QUEUE_KINDS.indexOf(left.kind) - QUEUE_KINDS.indexOf(right.kind)
    return byKind === 0 ? right.at - left.at : byKind
  })
}

/** What the item asks of the reader, in one line. */
export function queueLine(item: QueueItem, agentName: string): string {
  switch (item.kind) {
    case 'approval':
      return `${agentName} needs your approval`
    case 'waiting':
      return `${agentName} waits for your answer`
    case 'call':
      return `${agentName} missed a call from ${item.remoteE164}`
    case 'failed':
      return `${agentName} could not finish the work`
    case 'keypad':
      return `Callers entered a wrong keypad code ${item.failedAttempts} times`
  }
}

/** The name of the control that settles the item. */
export function queueAction(item: QueueItem): string {
  if (item.kind === 'call') return 'Call back'
  if (item.kind === 'keypad') return 'Clear the count'
  return item.kind === 'failed' ? 'Open run' : 'Open conversation'
}
