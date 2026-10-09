// The fold of the transcript rows of a Coding Session into the items
// that the session page shows.

import { describe, expect, it } from 'vitest'

import type { CodingSessionEventDto } from '../../api/client'
import { foldTranscript, type TranscriptItem } from './transcript'

function row(
  seq: number,
  kind: CodingSessionEventDto['kind'],
  payload: Record<string, unknown>,
): CodingSessionEventDto {
  return { seq, at: 1_000 + seq, kind, payload }
}

function toolCall(seq: number, fields: Record<string, unknown> = {}) {
  return row(seq, 'tool_call', {
    toolCallId: 'call-1',
    title: 'Run cargo test',
    kind: 'execute',
    status: 'pending',
    locations: [{ path: '/repo/Cargo.toml', line: 3 }],
    rawInput: { command: 'cargo test' },
    ...fields,
  })
}

function only<K extends TranscriptItem['kind']>(
  items: TranscriptItem[],
  kind: K,
): Extract<TranscriptItem, { kind: K }>[] {
  return items.filter((item): item is Extract<TranscriptItem, { kind: K }> => item.kind === kind)
}

describe('the messages', () => {
  it('reads a prompt as the sprite and an agent message as the harness', () => {
    const { items } = foldTranscript([
      row(1, 'prompt', { text: 'Fix the login bug', message_id: null }),
      row(2, 'agent_message', { text: 'I fixed it.', message_id: 'm1' }),
      row(3, 'thought', { text: 'The token expires.', message_id: null }),
    ])

    expect(items).toEqual([
      { kind: 'message', seq: 1, from: 'sprite', text: 'Fix the login bug', truncated: false },
      { kind: 'message', seq: 2, from: 'harness', text: 'I fixed it.', truncated: false },
      { kind: 'thought', seq: 3, text: 'The token expires.', truncated: false },
    ])
  })

  // A message row holds at most 16 KB, and the rest of the message goes
  // on in rows marked `continued`.
  it('joins a continued row to its message', () => {
    const { items } = foldTranscript([
      row(1, 'agent_message', { text: 'first half, ', message_id: 'm1' }),
      row(2, 'agent_message', { text: 'second half', message_id: 'm1', continued: true }),
    ])

    expect(items).toEqual([
      {
        kind: 'message',
        seq: 1,
        from: 'harness',
        text: 'first half, second half',
        truncated: false,
      },
    ])
  })
})

describe('the tool calls', () => {
  it('changes only the fields that an update holds', () => {
    const { items } = foldTranscript([
      toolCall(1),
      row(2, 'tool_call_update', { toolCallId: 'call-1', status: 'in_progress' }),
    ])

    expect(only(items, 'tool')).toEqual([
      {
        kind: 'tool',
        seq: 1,
        toolCallId: 'call-1',
        title: 'Run cargo test',
        toolKind: 'execute',
        status: 'in_progress',
        locations: [{ path: '/repo/Cargo.toml', line: 3 }],
        content: [],
        rawInput: { command: 'cargo test' },
        truncated: false,
      },
    ])
  })

  it('makes one item of a tool call and its two updates', () => {
    const { items } = foldTranscript([
      toolCall(1),
      row(2, 'tool_call_update', { toolCallId: 'call-1', status: 'in_progress' }),
      row(3, 'agent_message', { text: 'Running the tests.', message_id: 'm1' }),
      row(4, 'tool_call_update', {
        toolCallId: 'call-1',
        status: 'completed',
        content: [
          { type: 'content', content: { type: 'text', text: '12 passed' } },
          { type: 'diff', path: '/repo/src/login.rs', oldText: 'a', newText: 'b' },
          { type: 'terminal', terminalId: 't1' },
        ],
      }),
    ])

    const tools = only(items, 'tool')
    expect(tools).toHaveLength(1)
    expect(tools[0].status).toBe('completed')
    expect(tools[0].title).toBe('Run cargo test')
    expect(tools[0].content).toEqual([
      { type: 'text', text: '12 passed' },
      { type: 'diff', path: '/repo/src/login.rs', oldText: 'a', newText: 'b', truncated: false },
    ])
    expect(items.map((item) => item.kind)).toEqual(['tool', 'message'])
  })

  it('keeps the texts of a diff, a null old text, and the cut of its payload', () => {
    const { items } = foldTranscript([
      toolCall(1, {
        content: [{ type: 'diff', path: '/repo/new.rs', oldText: null, newText: 'fn main() {}' }],
      }),
      row(2, 'tool_call', {
        toolCallId: 'call-2',
        truncated: true,
        content: [{ type: 'diff', path: '/repo/big.rs', oldText: 'a', newText: 'b…' }],
      }),
    ])

    expect(only(items, 'tool').map((tool) => tool.content)).toEqual([
      [{ type: 'diff', path: '/repo/new.rs', oldText: null, newText: 'fn main() {}', truncated: false }],
      [{ type: 'diff', path: '/repo/big.rs', oldText: 'a', newText: 'b…', truncated: true }],
    ])
  })

  it('opens an item for an update whose tool call it has not read', () => {
    const { items } = foldTranscript([
      row(1, 'tool_call_update', { toolCallId: 'call-9', title: 'Read a file', status: 'completed' }),
    ])

    expect(only(items, 'tool')[0]).toMatchObject({
      toolCallId: 'call-9',
      title: 'Read a file',
      toolKind: null,
      status: 'completed',
    })
  })
})

describe('the plan', () => {
  it('is the last plan row, which replaces the earlier ones', () => {
    const { items, plan } = foldTranscript([
      row(1, 'plan', {
        entries: [{ content: 'Read the code', priority: 'medium', status: 'in_progress' }],
      }),
      row(2, 'plan', {
        entries: [
          { content: 'Read the code', priority: 'medium', status: 'completed' },
          { content: 'Fix the bug', priority: 'high', status: 'pending' },
        ],
      }),
    ])

    expect(items).toEqual([])
    expect(plan).toEqual([
      { content: 'Read the code', priority: 'medium', status: 'completed' },
      { content: 'Fix the bug', priority: 'high', status: 'pending' },
    ])
  })

  it('is absent when the harness sent none', () => {
    expect(foldTranscript([]).plan).toBeNull()
  })
})

describe('the asks', () => {
  it('makes one line of a permission and its decision', () => {
    const { items } = foldTranscript([
      row(1, 'permission', {
        ask_id: '7',
        tool_call_id: 'call-1',
        title: 'Run cargo test',
        kind: 'execute',
        locations: [],
        raw_input: { command: 'cargo test' },
        options: ['allow_once', 'reject_once'],
        waits_for: 'person',
      }),
      row(2, 'decision', { ask_id: '7', decision: 'allow_once', decider: 'person' }),
    ])

    expect(items).toEqual([
      {
        kind: 'permission',
        seq: 1,
        askId: '7',
        title: 'Run cargo test',
        toolKind: 'execute',
        waitsFor: 'person',
        decision: 'allow_once',
        decider: 'person',
        truncated: false,
      },
    ])
  })

  it('makes one line of a question and its answer', () => {
    const { items } = foldTranscript([
      row(1, 'question', { ask_id: '8', message: 'Which branch?', schema: {}, waits_for: null }),
      row(2, 'answer', { ask_id: '8', answer: 'cancel' }),
    ])

    expect(items).toEqual([
      {
        kind: 'question',
        seq: 1,
        askId: '8',
        message: 'Which branch?',
        waitsFor: null,
        answer: 'cancel',
        truncated: false,
      },
    ])
  })
})

describe('the Harness Mode', () => {
  it('makes one line of each mode row, with who changed the mode', () => {
    const { items } = foldTranscript([
      row(1, 'mode', { mode: 'default', name: 'Manual', by: 'pagis' }),
      row(2, 'mode', { mode: 'plan', name: 'Plan', by: 'agent' }),
      row(3, 'mode', { mode: 'acceptEdits', name: 'Accept edits', by: 'harness' }),
    ])

    expect(items).toEqual([
      { kind: 'mode', seq: 1, mode: 'default', name: 'Manual', by: 'pagis', truncated: false },
      { kind: 'mode', seq: 2, mode: 'plan', name: 'Plan', by: 'agent', truncated: false },
      {
        kind: 'mode',
        seq: 3,
        mode: 'acceptEdits',
        name: 'Accept edits',
        by: 'harness',
        truncated: false,
      },
    ])
  })

  it('names a mode with no name by its id', () => {
    const { items } = foldTranscript([row(1, 'mode', { mode: 'plan', by: 'harness' })])

    expect(only(items, 'mode')[0]?.name).toBe('plan')
  })
})

describe('the other rows', () => {
  it('makes a hairline of the end of a turn and no item of the usage', () => {
    const { items } = foldTranscript([
      row(1, 'usage', { used: 10, size: 100, cost: null }),
      row(2, 'turn_end', { stop_reason: 'end_turn' }),
    ])

    expect(items).toEqual([{ kind: 'turn_end', seq: 2, stopReason: 'end_turn', truncated: false }])
  })

  it('marks a row whose payload was cut', () => {
    const { items } = foldTranscript([
      toolCall(1),
      row(2, 'tool_call_update', {
        toolCallId: 'call-1',
        status: 'completed',
        truncated: true,
      }),
    ])

    expect(only(items, 'tool')[0].truncated).toBe(true)
  })

  it('places the items in the order of seq', () => {
    const { items } = foldTranscript([
      row(3, 'turn_end', { stop_reason: 'end_turn' }),
      row(1, 'prompt', { text: 'Go', message_id: null }),
      row(2, 'agent_message', { text: 'Done', message_id: null }),
    ])

    expect(items.map((item) => item.seq)).toEqual([1, 2, 3])
  })
})
