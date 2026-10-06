// Runs, the readable work record (docs/UI-DESIGN.md). This module
// holds every rule the two Runs screens read: what a run says it did,
// why it failed in plain words, and how one tool call summarises to a
// line. It knows nothing about React, so the wording is tested on
// its own.

import type { RunDto, RunEventDto } from '../../api/client'
import type { BadgeTone } from '../../primitives'
import { dayKey, formatDuration } from '../../timeline'

/** The states the daemon writes, in the order the filter chips read. */
export const RUN_STATES = [
  'queued',
  'running',
  'reflecting',
  'waiting_for_user',
  'waiting_for_approval',
  'completed',
  'failed',
  'canceled',
] as const

/** The state as a word and a hue. */
export function runStateBadge(state: string): { label: string; tone: BadgeTone } {
  switch (state) {
    case 'completed':
      return { label: 'Done', tone: 'working' }
    case 'failed':
      return { label: 'Failed', tone: 'failed' }
    case 'canceled':
      return { label: 'Stopped', tone: 'neutral' }
    case 'running':
      return { label: 'Working', tone: 'on-call' }
    case 'reflecting':
      return { label: 'Updating memory', tone: 'neutral' }
    case 'queued':
      return { label: 'Queued', tone: 'neutral' }
    case 'waiting_for_user':
      return { label: 'Waiting for you', tone: 'waiting' }
    case 'waiting_for_approval':
      return { label: 'Waiting for approval', tone: 'waiting' }
    default:
      return { label: state.replaceAll('_', ' '), tone: 'neutral' }
  }
}

/** What started the run, in words. */
export function triggerText(run: RunDto, channelName: string | null): string {
  const where = channelName === null ? '' : ` in ${channelName}`
  switch (run.trigger_kind) {
    case 'message':
      return `A message${where}`
    case 'event':
      return `An event${where}`
    case 'schedule':
      return `A schedule${where}`
    case 'agent':
      return `Another sprite${where}`
    case 'call':
      return `A phone call${where}`
    case 'mail':
      return `An email${where}`
    default:
      return `${run.trigger_kind.replaceAll('_', ' ')}${where}`
  }
}

/** The trigger inside a sentence, where the name it carries keeps its
 *  own capitals. */
export function triggerSentence(run: RunDto, channelName: string | null): string {
  const text = triggerText(run, channelName)
  return text.charAt(0).toLowerCase() + text.slice(1)
}

/** The recorded cause in plain words. The error remains the detail. */
const FAILURE_TEXT: Record<NonNullable<RunDto['failure_kind']>, string> = {
  agent_missing: 'Ended because the sprite could not be loaded',
  model_missing: 'Ended because the model could not be loaded',
  context_failed: 'Ended because the conversation could not be prepared',
  call_failed: 'Ended because the phone call failed',
  tool_failed: 'Ended because a tool failed',
  access_changed: 'Ended because access changed',
  publication_rejected: 'Ended because the reply was rejected',
  lease_failed: 'Ended because the computer could not be reserved',
  daemon_restarted: 'Ended because the daemon restarted',
  model_failed: 'Ended because the model request failed',
  turn_limit: 'Ended because the turn limit was reached',
  spend_cap_reached: 'Ended because the monthly spend cap was reached',
  unknown: 'Ended with an error',
}

export function failureText(run: RunDto): string | null {
  if (run.state !== 'failed') return null
  return run.failure_kind == null
    ? 'Ended with an error'
    : FAILURE_TEXT[run.failure_kind]
}

/** How long the run took, or what it is doing instead. */
export function runDuration(run: RunDto): string {
  if (run.duration_ms === null || run.duration_ms === undefined) return 'In progress'
  return formatDuration(run.duration_ms)
}

/** The runs of one calendar day. */
export interface RunDay {
  key: string
  label: string
  runs: RunDto[]
}

/** `Today`, `Yesterday`, or the date. */
export function dayLabel(at: number, now: number = Date.now()): string {
  const key = dayKey(at)
  if (key === dayKey(now)) return 'Today'
  if (key === dayKey(now - 86_400_000)) return 'Yesterday'
  return new Date(at).toLocaleDateString(undefined, {
    weekday: 'long',
    month: 'long',
    day: 'numeric',
  })
}

/** Group the runs by the day they were created, newest first. */
export function groupByDay(runs: readonly RunDto[], now: number = Date.now()): RunDay[] {
  const days = new Map<string, RunDay>()
  for (const run of [...runs].sort((left, right) => right.created_at - left.created_at)) {
    const key = dayKey(run.created_at)
    const day = days.get(key)
    if (day === undefined) {
      days.set(key, { key, label: dayLabel(run.created_at, now), runs: [run] })
    } else {
      day.runs.push(run)
    }
  }
  return [...days.values()]
}

/** The three things a reader narrows the record by. */
export interface RunFilters {
  agentId: string | null
  channelId: string | null
  state: string | null
}

export const NO_FILTERS: RunFilters = { agentId: null, channelId: null, state: null }

/** Does the run pass every filter that is set? */
export function matchesFilters(run: RunDto, filters: RunFilters): boolean {
  if (filters.agentId !== null && run.agent_id !== filters.agentId) return false
  if (filters.channelId !== null && run.channel_id !== filters.channelId) return false
  if (filters.state !== null && run.state !== filters.state) return false
  return true
}

/**
 * How many runs one chip would show.
 *
 * The count holds every other filter and replaces the chip's own, so
 * the number a chip carries is the number the reader gets by clicking
 * it — and it moves when another chip changes.
 */
export function chipCount(
  runs: readonly RunDto[],
  filters: RunFilters,
  dimension: keyof RunFilters,
  value: string | null,
): number {
  const narrowed = { ...filters, [dimension]: value }
  return runs.filter((run) => matchesFilters(run, narrowed)).length
}

/** One step of the run: one tool the agent called, with its result. */
export interface RunStep {
  event: RunEventDto
  name: string
  args: string
  result: string
  durationMs: number | null
  failed: boolean
  completed: RunEventDto | null
}

function payloadOf(event: RunEventDto): Record<string, unknown> {
  const payload = event.payload
  return payload !== null && typeof payload === 'object'
    ? (payload as Record<string, unknown>)
    : {}
}

/** The daemon writes the arguments as JSON text or as an object. */
function argumentsOf(payload: Record<string, unknown>): Record<string, unknown> {
  const raw = payload.arguments
  if (typeof raw === 'string') {
    try {
      const parsed: unknown = JSON.parse(raw)
      return parsed !== null && typeof parsed === 'object'
        ? (parsed as Record<string, unknown>)
        : { value: raw }
    } catch {
      return { value: raw }
    }
  }
  return raw !== null && typeof raw === 'object' ? (raw as Record<string, unknown>) : {}
}

/** The keys that name the call, not the work it asks for. */
const PLUMBING_KEYS = new Set(['call_id', 'id', 'status', 'type'])

/** The width one summary line holds. */
const SUMMARY_LINE = 90

function clamp(line: string): string {
  return line.length > SUMMARY_LINE ? `${line.slice(0, SUMMARY_LINE - 1)}…` : line
}

function compactValue(value: unknown): string {
  if (typeof value === 'string') return value
  if (typeof value === 'number' || typeof value === 'boolean') return String(value)
  if (Array.isArray(value)) return value.map(compactValue).filter(Boolean).join(', ')
  if (value !== null && typeof value === 'object') {
    const record = value as Record<string, unknown>
    // An action names itself; the rest of it is detail the raw event
    // holds.
    const name = record.type ?? record.name ?? record.action
    if (typeof name === 'string') return name
    return Object.keys(record).join(', ')
  }
  return ''
}

/** The arguments of one call as a single line. */
export function summariseArguments(event: RunEventDto): string {
  const args = argumentsOf(payloadOf(event))
  const parts: string[] = []
  for (const [key, value] of Object.entries(args)) {
    if (PLUMBING_KEYS.has(key)) continue
    const text = compactValue(value)
    if (text !== '') parts.push(`${key}: ${text}`)
  }
  return parts.length === 0 ? 'No argument' : clamp(parts.join(' · '))
}

/** What the call gave back, as a single line. */
export function summariseResult(completed: RunEventDto | null): string {
  if (completed === null) return 'No result'
  const payload = payloadOf(completed)
  if (payload.ok === false || typeof payload.error === 'string') {
    const error = typeof payload.error === 'string' ? payload.error : 'no reason given'
    return clamp(`Failed: ${error}`)
  }
  const shots = Array.isArray(payload.artifact_ids) ? payload.artifact_ids.length : 0
  if (shots === 1) return 'Done, with 1 screenshot'
  if (shots > 1) return `Done, with ${shots} screenshots`
  return 'Done'
}

/**
 * Every tool call of the run, joined to its completion.
 *
 * A completion follows its call and names the same tool. One completion
 * settles one call, so a tool the agent called twice reads as two
 * steps, in order.
 */
export function runSteps(events: readonly RunEventDto[]): RunStep[] {
  const open: RunStep[] = []
  const steps: RunStep[] = []
  for (const event of events) {
    const payload = payloadOf(event)
    const name = typeof payload.name === 'string' ? payload.name : event.event_type
    if (event.event_type === 'tool.called') {
      const step: RunStep = {
        event,
        name,
        args: summariseArguments(event),
        result: summariseResult(null),
        durationMs: null,
        failed: false,
        completed: null,
      }
      open.push(step)
      steps.push(step)
      continue
    }
    if (event.event_type !== 'tool.completed') continue
    const index = open.findIndex((step) => step.name === name)
    if (index === -1) continue
    const step = open.splice(index, 1)[0]
    step.completed = event
    step.result = summariseResult(event)
    step.durationMs =
      typeof payload.duration_ms === 'number' ? payload.duration_ms : null
    step.failed = payload.ok === false || typeof payload.error === 'string'
  }
  return steps
}

/** One model request of the run: its size, and the error when it failed. */
export interface ModelRequest {
  event: RunEventDto
  /** `phase:phase_request`, the key that joins the request to its
   *  completion and to its capture. */
  key: string
  label: string
  summary: string
  /** The call failed, or the budget check rejected it before the call. */
  failed: boolean
  error: string | null
}

const PHASE_LABEL: Record<string, string> = {
  reply: 'Turn',
  compaction: 'Compaction',
  reflection: 'Reflection',
}

function count(value: unknown, noun: string): string | null {
  if (typeof value !== 'number') return null
  return `${value.toLocaleString()} ${noun}${value === 1 ? '' : 's'}`
}

function requestSummary(
  requested: Record<string, unknown>,
  completed: Record<string, unknown> | null,
): string {
  const candidates = Array.isArray(requested.model_candidates)
    ? requested.model_candidates.filter((candidate) => typeof candidate === 'string')
    : []
  const served =
    completed !== null && typeof completed.provider === 'string' && typeof completed.model === 'string'
      ? `${completed.provider}/${completed.model}`
      : null
  const route =
    served ?? (candidates.length === 1 ? candidates[0] : String(requested.model_alias ?? 'model'))
  const estimate = requested.estimated_input_tokens
  const allowance = requested.input_allowance
  const output = requested.max_output_tokens
  const parts = [
    route,
    typeof estimate === 'number' && typeof allowance === 'number'
      ? `about ${estimate.toLocaleString()} of ${allowance.toLocaleString()} input tokens`
      : null,
    typeof output === 'number' ? `max output ${output.toLocaleString()}` : 'max output not set',
    count(requested.messages, 'message'),
    count(requested.tools, 'tool'),
    typeof requested.images === 'number' && requested.images > 0
      ? count(requested.images, 'image')
      : null,
  ]
  return parts.filter((part) => part !== null).join(' · ')
}

/**
 * Every model request of the run, joined to its completion.
 *
 * The daemon records `model.requested` before the call and
 * `model.completed` after it, with the same phase and request number.
 */
export function modelRequests(events: readonly RunEventDto[]): ModelRequest[] {
  const completions = new Map<string, Record<string, unknown>>()
  for (const event of events) {
    if (event.event_type !== 'model.completed') continue
    const payload = payloadOf(event)
    completions.set(`${String(payload.phase)}:${String(payload.phase_request)}`, payload)
  }
  return events
    .filter((event) => event.event_type === 'model.requested')
    .map((event) => {
      const payload = payloadOf(event)
      const phase = String(payload.phase)
      const number = typeof payload.phase_request === 'number' ? payload.phase_request + 1 : 1
      const key = `${phase}:${String(payload.phase_request)}`
      const completed = completions.get(key) ?? null
      return {
        event,
        key,
        label: `${PHASE_LABEL[phase] ?? phase} ${number}`,
        summary: requestSummary(payload, completed),
        failed:
          completed !== null &&
          (completed.outcome === 'failed' || completed.outcome === 'rejected'),
        error: completed !== null && typeof completed.error === 'string' ? completed.error : null,
      }
    })
}

/** How long one step took. */
export function stepDuration(step: RunStep): string {
  if (step.durationMs === null) return '—'
  if (step.durationMs < 1_000) return `${step.durationMs} ms`
  return formatDuration(step.durationMs)
}
