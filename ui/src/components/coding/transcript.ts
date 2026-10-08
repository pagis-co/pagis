// The transcript of a Coding Session as the session page shows it: a
// pure fold of the stored rows into display items and the plan
// (ADR-0033).
//
// Each payload keeps the field names of the harness protocol. A tool
// call opens one item, and each update of it changes only the fields
// that the update holds. Each plan replaces the whole plan, so the last
// plan row is the plan. A permission and its decision, and a question
// and its answer, are one line each. The usage makes no item, because
// the record holds it.

import type { CodingSessionEventDto } from '../../api/client'

/** A file that a tool call reads or changes. */
export interface ToolLocation {
  path: string
  line?: number
}

/** What a tool call gave back. A diff shows its path only. */
export type ToolContent = { type: 'text'; text: string } | { type: 'diff'; path: string }

export interface PlanEntry {
  content: string
  priority: string
  status: string
}

export type TranscriptItem =
  | { kind: 'message'; seq: number; from: 'sprite' | 'harness'; text: string; truncated: boolean }
  | { kind: 'thought'; seq: number; text: string; truncated: boolean }
  | {
      kind: 'tool'
      seq: number
      toolCallId: string
      title: string
      toolKind: string | null
      status: string
      locations: ToolLocation[]
      content: ToolContent[]
      rawInput: unknown
      truncated: boolean
    }
  | {
      kind: 'permission'
      seq: number
      askId: string
      title: string | null
      toolKind: string | null
      waitsFor: string | null
      decision: string | null
      decider: string | null
      truncated: boolean
    }
  | {
      kind: 'question'
      seq: number
      askId: string
      message: string
      waitsFor: string | null
      answer: unknown
      truncated: boolean
    }
  | { kind: 'turn_end'; seq: number; stopReason: string; truncated: boolean }

export interface Transcript {
  items: TranscriptItem[]
  /** The last plan of the harness, or `null` when it sent none. */
  plan: PlanEntry[] | null
}

type Payload = Record<string, unknown>
type ToolItem = Extract<TranscriptItem, { kind: 'tool' }>
type PermissionItem = Extract<TranscriptItem, { kind: 'permission' }>
type QuestionItem = Extract<TranscriptItem, { kind: 'question' }>

function text(value: unknown): string | null {
  return typeof value === 'string' ? value : null
}

function locationsOf(value: unknown): ToolLocation[] {
  if (!Array.isArray(value)) return []
  return value.flatMap((location: unknown) => {
    if (location === null || typeof location !== 'object') return []
    const { path, line } = location as Payload
    if (typeof path !== 'string') return []
    return [typeof line === 'number' ? { path, line } : { path }]
  })
}

function contentOf(value: unknown): ToolContent[] {
  if (!Array.isArray(value)) return []
  return value.flatMap((entry: unknown): ToolContent[] => {
    if (entry === null || typeof entry !== 'object') return []
    const item = entry as Payload
    if (item.type === 'diff' && typeof item.path === 'string') {
      return [{ type: 'diff', path: item.path }]
    }
    if (item.type === 'content' && item.content !== null && typeof item.content === 'object') {
      const block = item.content as Payload
      if (block.type === 'text' && typeof block.text === 'string') {
        return [{ type: 'text', text: block.text }]
      }
    }
    return []
  })
}

function planOf(value: unknown): PlanEntry[] {
  if (!Array.isArray(value)) return []
  return value.flatMap((entry: unknown) => {
    if (entry === null || typeof entry !== 'object') return []
    const { content, priority, status } = entry as Payload
    if (typeof content !== 'string') return []
    return [
      {
        content,
        priority: text(priority) ?? 'medium',
        status: text(status) ?? 'pending',
      },
    ]
  })
}

/** Change a tool item by the fields that a tool call or an update
 *  holds. A field that the payload does not hold stays as it was. */
function applyTool(item: ToolItem, payload: Payload) {
  if ('title' in payload) item.title = text(payload.title) ?? item.title
  if ('kind' in payload) item.toolKind = text(payload.kind)
  if ('status' in payload) item.status = text(payload.status) ?? item.status
  if ('locations' in payload) item.locations = locationsOf(payload.locations)
  if ('content' in payload) item.content = contentOf(payload.content)
  if ('rawInput' in payload) item.rawInput = payload.rawInput
  if (payload.truncated === true) item.truncated = true
}

/** Fold the rows of a transcript, in any order, into its items in the
 *  order of `seq`, and its plan. */
export function foldTranscript(rows: readonly CodingSessionEventDto[]): Transcript {
  const items: TranscriptItem[] = []
  const tools = new Map<string, ToolItem>()
  const permissions = new Map<string, PermissionItem>()
  const questions = new Map<string, QuestionItem>()
  let plan: PlanEntry[] | null = null

  for (const { seq, kind, payload } of [...rows].sort((a, b) => a.seq - b.seq)) {
    const truncated = payload.truncated === true
    switch (kind) {
      case 'prompt':
      case 'agent_message':
      case 'thought': {
        const body = text(payload.text) ?? ''
        const last = items.at(-1)
        const continues =
          payload.continued === true &&
          last !== undefined &&
          (last.kind === 'message' || last.kind === 'thought') &&
          (last.kind === 'thought') === (kind === 'thought')
        if (continues) {
          last.text += body
          last.truncated ||= truncated
        } else if (kind === 'thought') {
          items.push({ kind: 'thought', seq, text: body, truncated })
        } else {
          const from = kind === 'prompt' ? 'sprite' : 'harness'
          items.push({ kind: 'message', seq, from, text: body, truncated })
        }
        break
      }
      case 'tool_call':
      case 'tool_call_update': {
        const id = text(payload.toolCallId)
        if (id === null) break
        let item = tools.get(id)
        if (item === undefined) {
          item = {
            kind: 'tool',
            seq,
            toolCallId: id,
            title: '',
            toolKind: null,
            status: 'pending',
            locations: [],
            content: [],
            rawInput: undefined,
            truncated: false,
          }
          tools.set(id, item)
          items.push(item)
        }
        applyTool(item, payload)
        break
      }
      case 'plan':
        plan = planOf(payload.entries)
        break
      case 'permission': {
        const askId = text(payload.ask_id) ?? String(seq)
        const item: PermissionItem = {
          kind: 'permission',
          seq,
          askId,
          title: text(payload.title),
          toolKind: text(payload.kind),
          waitsFor: text(payload.waits_for),
          decision: null,
          decider: null,
          truncated,
        }
        permissions.set(askId, item)
        items.push(item)
        break
      }
      case 'decision': {
        const item = permissions.get(text(payload.ask_id) ?? '')
        if (item === undefined) break
        item.decision = text(payload.decision)
        item.decider = text(payload.decider)
        item.truncated ||= truncated
        break
      }
      case 'question': {
        const askId = text(payload.ask_id) ?? String(seq)
        const item: QuestionItem = {
          kind: 'question',
          seq,
          askId,
          message: text(payload.message) ?? '',
          waitsFor: text(payload.waits_for),
          answer: null,
          truncated,
        }
        questions.set(askId, item)
        items.push(item)
        break
      }
      case 'answer': {
        const item = questions.get(text(payload.ask_id) ?? '')
        if (item === undefined) break
        item.answer = payload.answer ?? null
        item.truncated ||= truncated
        break
      }
      case 'turn_end':
        items.push({
          kind: 'turn_end',
          seq,
          stopReason: text(payload.stop_reason) ?? '',
          truncated,
        })
        break
      case 'usage':
        break
    }
  }
  return { items, plan }
}
