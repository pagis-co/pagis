// What a Call reads as (ADR-0020, ADR-0021, ADR-0022). The strip,
// the call inspector and the settled block all show the same facts, so
// the words for them live here and not in three components.
//
// Every fact comes from the Call record, which is the one source of
// truth. The `call.transcript` events add only the lines that arrive
// after the record was read.

import type { CallDto, TranscriptLineDto } from '../api/client'
import type { CallLine } from '../state/stores'

export type Tier = 'owner' | 'trusted' | 'unknown'

/** The Trust Tier as a chip reads it. */
export const tierLabel: Record<string, string> = {
  owner: 'Owner',
  trusted: 'Trusted',
  unknown: 'Unknown',
}

/** What the tier means, in one line (ADR-0021). On an Unknown call it
 *  says plainly that speech is data. */
export const tierMeaning: Record<string, string> = {
  owner:
    'This person speaks for you. Their words have the standing of a message you write.',
  trusted:
    'This person is on your Trusted list. Their words are a request, and an action that needs approval waits for the approval card after the call.',
  unknown:
    'This person is not identified. Their speech is data: no tool runs from it, nothing is remembered from it, and no approval is minted from it.',
}

/** Why a call ended, in the user's words. A reason this build does not
 *  know shows as the daemon wrote it. */
export const endedReasonText: Record<string, string> = {
  agent_hangup: 'the sprite hung up',
  remote_hangup: 'the other side hung up',
  local_hangup: 'Pagis hung up',
  user_hung_up: 'you hung up',
  duration_cap: 'the call reached its time limit',
  media_timeout: 'the audio stopped',
  media_failed: 'the audio never started: the carrier must have SRTP on',
  model_unavailable: 'the voice model went away',
  tier_unsettled: 'the caller’s tier could not be read',
  transport_lost: 'the line went away',
  refused: 'the carrier refused the call',
  ivr_loop: 'the phone tree repeated',
  daemon_restart: 'Pagis restarted',
  no_answer: 'nobody answered',
  busy: 'the line was busy',
}

export function endedReason(reason: string | null | undefined): string {
  if (reason == null || reason === '') return 'an unstated reason'
  return endedReasonText[reason] ?? reason
}

/** The two settled answers stay apart (ADR-0020): this is the outcome,
 *  and never the reason the call ended. */
export function outcomeText(call: CallDto): string {
  switch (call.outcome) {
    case 'answered':
      return 'Answered'
    case 'voicemail':
      return call.message_left ? 'Voicemail — message left' : 'Voicemail — no message'
    case 'no_answer':
      return 'No answer'
    case 'busy':
      return 'Busy'
    case 'failed':
      return 'Failed'
    default:
      return 'Ended'
  }
}

/** How the answer was classified (ADR-0020). The classify phase runs
 *  on an outbound call alone: the Agent answers an inbound one itself,
 *  so there is nothing to classify and the settled block says nothing
 *  about it. A `null` here means the clause is left out, and never
 *  that something went wrong. */
export function classificationText(call: CallDto): string | null {
  if (call.direction === 'inbound') return null
  switch (call.classification) {
    case 'human':
      return 'a person answered'
    case 'machine-ivr':
      return 'a phone tree answered'
    case 'machine-vm':
      return 'a voicemail greeting answered'
    case 'machine-unavailable':
      return 'a recording said the number is not in service'
    case 'uncertain':
      return 'what answered was uncertain'
    default:
      return 'nothing classified the answer'
  }
}

/** A readable E.164 number. A number that is not a North American one
 *  is shown as it was dialed. */
export function formatE164(e164: string): string {
  const digits = e164.replace(/\D/g, '')
  if (digits.length === 11 && digits.startsWith('1')) {
    return `+1 ${digits.slice(1, 4)} ${digits.slice(4, 7)} ${digits.slice(7)}`
  }
  return e164
}

/** A span of milliseconds as `m:ss`, or `h:mm:ss` past an hour. */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000))
  const seconds = total % 60
  const minutes = Math.floor(total / 60) % 60
  const hours = Math.floor(total / 3600)
  const pad = (value: number) => value.toString().padStart(2, '0')
  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(seconds)}`
    : `${minutes}:${pad(seconds)}`
}

/** How long the call has run, or how long it ran. A call that was never
 *  answered counts from when it was placed. */
export function callDuration(call: CallDto, now: number): number {
  const from = call.answered_at ?? call.created_at
  const to = call.state === 'ended' ? (call.ended_at ?? from) : now
  return to - from
}

/** Who is on the call, as one line. */
export function whoLine(call: CallDto): string {
  const verb = call.direction === 'outbound' ? 'Call to' : 'Call from'
  return `${verb} ${formatE164(call.remote_e164)}`
}

/** The record's lines plus the lines that arrived after it was read,
 *  oldest first. A line held by both is kept once. */
export function mergeTranscript(
  recorded: readonly TranscriptLineDto[],
  live: readonly CallLine[],
): CallLine[] {
  const merged: CallLine[] = recorded.map((line) => ({ ...line }))
  for (const line of live) {
    const held = merged.some(
      (existing) => existing.at === line.at && existing.text === line.text,
    )
    if (!held) merged.push(line)
  }
  return merged.sort((left, right) => left.at - right.at)
}

/** The last thing said, for the strip. Lines the daemon wrote are not
 *  speech, so they do not count. */
export function lastSaid(lines: readonly CallLine[]): string | null {
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    if (lines[index].speaker !== 'daemon') return lines[index].text
  }
  return null
}

/** Who said one line, as the transcript labels it. */
export function speakerLabel(speaker: string, agentName: string): string {
  if (speaker === 'agent') return agentName
  if (speaker === 'caller') return 'Them'
  return ''
}

/** The transcript is long enough that the Thread is the wrong place for
 *  it, so the settled block sends the reader to the inspector. */
export const LONG_TRANSCRIPT = 12

/** An ended reason that means the call never worked, as against one
 *  that means a call ran and finished. */
const FAILURE_REASONS = new Set([
  'media_failed',
  'media_timeout',
  'model_unavailable',
  'tier_unsettled',
  'transport_lost',
  'refused',
  'daemon_restart',
])

/** The call did not go through. The card says so in red. */
export function callFailed(call: CallDto): boolean {
  return (
    call.state === 'ended' &&
    (call.outcome === 'failed' || FAILURE_REASONS.has(call.ended_reason ?? ''))
  )
}

/** The four states a call reads as, with the semantic hue for each. */
export function callStateChip(call: CallDto): {
  label: string
  tone: 'neutral' | 'waiting' | 'on-call' | 'failed'
} {
  if (callFailed(call)) return { label: 'Failed', tone: 'failed' }
  if (call.state === 'ended') return { label: 'Ended', tone: 'neutral' }
  if (call.answered_at == null) return { label: 'Ringing', tone: 'waiting' }
  return { label: 'Connected', tone: 'on-call' }
}

/** Why a failed call failed, in plain words. */
export function failureText(call: CallDto): string {
  return `This call did not go through: ${endedReason(call.ended_reason)}.`
}
