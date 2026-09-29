// The conversation view model: one header per author per five
// minutes, the tool loop folded under the reply it produced, the "Done"
// termination hidden, and the day, unread and system strips.

import { describe, expect, it } from 'vitest'

import {
  buildConversation,
  formatDuration,
  isProgressRow,
  type ConversationItem,
  type TimelineRow,
} from './timeline'

const MINUTE = 60 * 1000

function row(overrides: Partial<TimelineRow>): TimelineRow {
  return {
    key: 'msg-1',
    kind: 'message',
    authorKind: 'agent',
    authorAgentId: 'sage',
    createdAt: Date.parse('2026-09-05T10:00:00Z'),
    sendState: 'sent',
    status: 'complete',
    runId: null,
    blocks: [{ type: 'markdown', text: 'hi' }],
    text: 'hi',
    completedAt: null,
    replyCount: 0,
    lastReplyAt: null,
    replyAuthors: [],
    ...overrides,
  }
}

/** A tool-loop row: one `progress` block and nothing else. */
function progressRow(overrides: Partial<TimelineRow>): TimelineRow {
  const text = overrides.text ?? 'Done'
  const runId = overrides.runId ?? 'run-1'
  return row({
    key: 'msg-p',
    runId,
    text,
    blocks: [{ type: 'progress', run_id: runId, text }],
    ...overrides,
  })
}

function messages(items: ConversationItem[]) {
  return items.filter((item) => item.kind === 'message')
}

describe('isProgressRow', () => {
  it('knows the tool loop by its blocks, not by its text', () => {
    expect(isProgressRow(progressRow({}))).toBe(true)
    expect(isProgressRow(row({ text: 'Done' }))).toBe(false)
  })
})

describe('buildConversation', () => {
  it('gives three consecutive messages of one author one header', () => {
    const at = Date.parse('2026-09-05T10:00:00Z')
    const items = messages(
      buildConversation([
        row({ key: 'm1', createdAt: at }),
        row({ key: 'm2', createdAt: at + MINUTE }),
        row({ key: 'm3', createdAt: at + 2 * MINUTE }),
      ]),
    )

    expect(items.map((item) => item.grouped)).toEqual([false, true, true])
  })

  it('opens a new header after five minutes or another author', () => {
    const at = Date.parse('2026-09-05T10:00:00Z')
    const items = messages(
      buildConversation([
        row({ key: 'm1', createdAt: at }),
        row({ key: 'm2', createdAt: at + 6 * MINUTE }),
        row({ key: 'm3', authorAgentId: 'clown', createdAt: at + 7 * MINUTE }),
        row({ key: 'm4', authorAgentId: 'clown', createdAt: at + 7 * MINUTE }),
      ]),
    )

    expect(items.map((item) => item.grouped)).toEqual([false, false, false, true])
  })

  it('hides the "Done" termination and folds the run under the reply', () => {
    const at = Date.parse('2026-09-05T10:00:00Z')
    // The progress row spans the run; the reply settles inside it.
    const items = buildConversation([
      progressRow({
        key: 'p1',
        createdAt: at,
        completedAt: at + 51_000,
        runId: 'run-1',
      }),
      row({
        key: 'm1',
        runId: 'run-1',
        createdAt: at + 1000,
        completedAt: at + 49_000,
      }),
    ])

    expect(items.map((item) => item.key)).toEqual([
      'day-' + new Date(at).toDateString(),
      'm1',
    ])
    const message = messages(items)[0]
    expect(message.work).toEqual({
      runId: 'run-1',
      startedAt: at,
      endedAt: at + 51_000,
      outcome: 'Done',
    })
  })

  it('shows reflection after the reply and hides its uninformative Done end', () => {
    const at = Date.parse('2026-09-05T10:00:00Z')
    const reply = row({
      key: 'm1',
      runId: 'run-1',
      createdAt: at + 1000,
      completedAt: at + 2000,
    })

    const reflecting = buildConversation([
      progressRow({ key: 'p1', runId: 'run-1', text: 'Updating memory', createdAt: at }),
      reply,
    ])
    expect(reflecting.map((item) => item.kind)).toEqual(['day', 'message', 'reflecting'])

    const completed = buildConversation([
      progressRow({ key: 'p1', runId: 'run-1', text: 'Done', createdAt: at }),
      reply,
    ])
    expect(completed.map((item) => item.kind)).toEqual(['day', 'message'])
  })

  it('drops a "Done" row that has no reply, and keeps a live line', () => {
    const items = buildConversation([
      progressRow({ key: 'p1', runId: 'run-1', text: 'Done' }),
      progressRow({ key: 'p2', runId: 'run-2', text: 'Running `git status`\u2026' }),
      progressRow({ key: 'p3', runId: 'run-3', text: 'Failed' }),
    ])

    // The live line is the Working row; the failure is a quiet line.
    expect(items.map((item) => item.kind)).toEqual(['day', 'working', 'outcome'])
    expect(items.map((item) => item.key)).toEqual(['day-' + new Date(Date.parse('2026-09-05T10:00:00Z')).toDateString(), 'p2', 'p3'])
  })

  it('says a run the user stopped in a quiet line of its own', () => {
    const items = buildConversation([
      progressRow({ key: 'p1', runId: 'run-1', text: 'Stopped' }),
    ])

    const outcome = items.find((item) => item.kind === 'outcome')
    expect(outcome?.kind === 'outcome' && outcome.outcome).toBe('Stopped')
  })

  it('keeps the Working row out of the message flow', () => {
    const items = buildConversation([
      row({ key: 'm1' }),
      progressRow({ key: 'p1', runId: 'run-1', text: 'Thinking\u2026' }),
      row({ key: 'm2' }),
    ])

    // The Working row breaks the group: the author speaks under a
    // header again after it.
    expect(messages(items).map((item) => item.grouped)).toEqual([false, false])
  })

  it('opens a divider on every new day', () => {
    const first = Date.parse('2026-09-04T10:00:00Z')
    const second = Date.parse('2026-09-05T10:00:00Z')
    const items = buildConversation([
      row({ key: 'm1', createdAt: first }),
      row({ key: 'm2', createdAt: second }),
    ])

    expect(items.map((item) => item.kind)).toEqual([
      'day',
      'message',
      'day',
      'message',
    ])
    // A day break also opens a new header.
    expect(messages(items).map((item) => item.grouped)).toEqual([false, false])
  })

  it('marks the first unread row once, and none when all are read', () => {
    const at = Date.parse('2026-09-05T10:00:00Z')
    const rows = [
      row({ key: 'm1', createdAt: at }),
      row({ key: 'm2', createdAt: at + 1000 }),
      row({ key: 'm3', createdAt: at + 2000 }),
    ]

    const marked = buildConversation(rows, { lastReadAt: at })
    const index = marked.findIndex((item) => item.kind === 'unread')
    expect(marked[index + 1].key).toBe('m2')
    expect(marked.filter((item) => item.kind === 'unread')).toHaveLength(1)

    const read = buildConversation(rows, { lastReadAt: at + 2000 })
    expect(read.some((item) => item.kind === 'unread')).toBe(false)
  })

  it('takes a system event out of the message flow', () => {
    const at = Date.parse('2026-09-05T10:00:00Z')
    const items = buildConversation([
      row({ key: 'm1', createdAt: at }),
      row({ key: 's1', authorKind: 'system', authorAgentId: null, createdAt: at }),
      row({ key: 'm2', createdAt: at }),
    ])

    expect(items.map((item) => item.kind)).toEqual([
      'day',
      'message',
      'system',
      'message',
    ])
    // The strip breaks the group: the author speaks again under a header.
    expect(messages(items).map((item) => item.grouped)).toEqual([false, false])
  })
})

describe('formatDuration', () => {
  it('reads a long run in minutes', () => {
    expect(formatDuration(64_000)).toBe('1 min 4 s')
  })
})
