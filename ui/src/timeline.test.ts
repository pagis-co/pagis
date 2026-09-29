// The optimistic-send reconcile rules: pending rows show
// until a server row carries the same pending_id; failed rows persist
// with retry; the server page (newest first) renders oldest first. The
// live run progress overrides its block's text, and renders as a
// row of its own until the server page carries it.

import { describe, expect, it } from 'vitest'

import type { RunProgress } from './state/stores'
import {
  formatClock,
  mergeTimeline,
  plainText,
  threadScope,
  type PendingSend,
  type TimelineMessage,
} from './timeline'

function serverItem(overrides: Partial<TimelineMessage>): TimelineMessage {
  return {
    kind: 'message',
    id: 'msg-1',
    channel_id: 'ch-1',
    author_kind: 'user',
    status: 'complete',
    blocks: [{ type: 'markdown', text: 'hi' }],
    text_content: 'hi',
    created_at: 1000,
    reply_count: 0,
    ...overrides,
  }
}

function runProgress(overrides: Partial<RunProgress>): RunProgress {
  return {
    run_id: 'run-1',
    message_id: 'msg-p1',
    agent_id: 'ag-1',
    seq: 2,
    text: 'Running `git status`…',
    ...overrides,
  }
}

function pendingSend(overrides: Partial<PendingSend>): PendingSend {
  return {
    pending_id: 'p-1',
    text: 'draft',
    created_at: 2000,
    state: 'pending',
    ...overrides,
  }
}

describe('mergeTimeline', () => {
  it('renders the newest-first server page oldest first', () => {
    const rows = mergeTimeline(
      [
        serverItem({ id: 'msg-2', created_at: 2000 }),
        serverItem({ id: 'msg-1', created_at: 1000 }),
      ],
      [],
    )
    expect(rows.map((r) => r.key)).toEqual(['msg-1', 'msg-2'])
    expect(rows.every((r) => r.sendState === 'sent')).toBe(true)
  })

  it('appends pending sends after the server rows', () => {
    const rows = mergeTimeline(
      [serverItem({ id: 'msg-1' })],
      [pendingSend({ pending_id: 'p-1' })],
    )
    expect(rows.map((r) => r.key)).toEqual(['msg-1', 'p-1'])
    expect(rows[1].sendState).toBe('pending')
    expect(rows[1].blocks).toEqual([{ type: 'markdown', text: 'draft' }])
  })

  it('drops a pending send once a server row carries its pending_id', () => {
    const rows = mergeTimeline(
      [serverItem({ id: 'msg-1', pending_id: 'p-1' })],
      [pendingSend({ pending_id: 'p-1' })],
    )
    expect(rows.map((r) => r.key)).toEqual(['msg-1'])
  })

  it('keeps failed sends visible', () => {
    const rows = mergeTimeline([], [pendingSend({ state: 'failed' })])
    expect(rows).toHaveLength(1)
    expect(rows[0].sendState).toBe('failed')
  })

  it('overrides a streaming row with its live buffer', () => {
    const rows = mergeTimeline(
      [
        serverItem({
          id: 'msg-2',
          author_kind: 'agent',
          status: 'streaming',
          run_id: 'run-1',
          text_content: '',
          blocks: [{ type: 'markdown', text: '' }],
        }),
        serverItem({ id: 'msg-1' }),
      ],
      [],
      {
        'msg-2': {
          message_id: 'msg-2',
          run_id: 'run-1',
          agent_id: 'ag-1',
          seq: 2,
          text: 'Hel',
        },
      },
    )
    expect(rows[1].status).toBe('streaming')
    expect(rows[1].runId).toBe('run-1')
    expect(rows[1].text).toBe('Hel')
    expect(rows[1].blocks).toEqual([{ type: 'markdown', text: 'Hel' }])
  })

  it('renders a live stream without a server row as a synthetic agent row', () => {
    const rows = mergeTimeline([serverItem({ id: 'msg-1' })], [], {
      'msg-9': {
        message_id: 'msg-9',
        run_id: 'run-1',
        agent_id: 'ag-1',
        seq: 1,
        text: 'ping',
      },
    })
    expect(rows.map((r) => r.key)).toEqual(['msg-1', 'msg-9'])
    expect(rows[1].authorKind).toBe('agent')
    expect(rows[1].status).toBe('streaming')
    expect(rows[1].text).toBe('ping')
  })

  it('carries the thread rollup into the row', () => {
    const rows = mergeTimeline(
      [serverItem({ id: 'msg-1', reply_count: 3, last_reply_at: 4000 })],
      [],
    )
    expect(rows[0].replyCount).toBe(3)
    expect(rows[0].lastReplyAt).toBe(4000)
  })

  it('carries the reply authors into the row', () => {
    const rows = mergeTimeline(
      [
        serverItem({
          id: 'msg-1',
          reply_count: 2,
          last_reply_at: 4000,
          reply_authors: [
            { author_kind: 'agent', author_agent_id: 'ag-2' },
            { author_kind: 'user', author_agent_id: null },
          ],
        }),
      ],
      [pendingSend({ pending_id: 'p-1' })],
    )
    expect(rows[0].replyAuthors).toEqual([
      { authorKind: 'agent', agentId: 'ag-2' },
      { authorKind: 'user', agentId: null },
    ])
    expect(rows[1].replyAuthors).toEqual([])
  })

  it('defaults the rollup to zero for replies and pending rows', () => {
    const reply = serverItem({ id: 'msg-1', parent_message_id: 'root-1' })
    delete (reply as { reply_count?: number }).reply_count
    const rows = mergeTimeline([reply], [pendingSend({ pending_id: 'p-1' })])
    expect(rows[0].replyCount).toBe(0)
    expect(rows[0].lastReplyAt).toBeNull()
    expect(rows[1].replyCount).toBe(0)
  })

  it('keeps a failed agent row with its persisted partial', () => {
    const rows = mergeTimeline(
      [
        serverItem({
          id: 'msg-2',
          author_kind: 'agent',
          status: 'failed',
          run_id: 'run-1',
          text_content: 'Partial ',
          blocks: [{ type: 'markdown', text: 'Partial ' }],
        }),
      ],
      [],
      {},
    )
    expect(rows[0].status).toBe('failed')
    expect(rows[0].text).toBe('Partial ')
  })

  it('renders a pointer item as a pointer row in server order', () => {
    const rows = mergeTimeline(
      [
        serverItem({ id: 'msg-3', created_at: 3000 }),
        {
          kind: 'pointer',
          id: 'msg-2',
          channel_id: 'ch-2',
          channel_title: 'Sage ↔ Scout',
          agent_id: 'agent-1',
          preview: 'Check the logs',
          created_at: 2000,
        },
        serverItem({ id: 'msg-1', created_at: 1000 }),
      ],
      [],
    )
    expect(rows.map((r) => [r.key, r.kind])).toEqual([
      ['msg-1', 'message'],
      ['msg-2', 'pointer'],
      ['msg-3', 'message'],
    ])
    expect(rows[1].pointer).toEqual({
      channelId: 'ch-2',
      channelTitle: 'Sage ↔ Scout',
    })
    expect(rows[1].text).toBe('Check the logs')
  })

  it('carries the author agent id so a row can name its speaker', () => {
    const rows = mergeTimeline(
      [
        serverItem({
          id: 'msg-1',
          author_kind: 'agent',
          author_agent_id: 'ag-1',
        }),
      ],
      [],
    )
    expect(rows[0].authorAgentId).toBe('ag-1')
  })

  it('names a live stream that has no server row yet', () => {
    const rows = mergeTimeline([], [], {
      'msg-9': {
        message_id: 'msg-9',
        run_id: 'run-1',
        agent_id: 'ag-1',
        seq: 1,
        text: 'thin',
      },
    })
    expect(rows[0].authorAgentId).toBe('ag-1')
  })

  it('shows the live progress line in place of the block text', () => {
    const rows = mergeTimeline(
      [
        serverItem({
          id: 'msg-p1',
          author_kind: 'agent',
          run_id: 'run-1',
          status: 'streaming',
          blocks: [{ type: 'progress', run_id: 'run-1', text: 'Thinking…' }],
          text_content: 'Thinking…',
        }),
      ],
      [],
      {},
      { 'run-1': runProgress({}) },
    )
    expect(rows[0].blocks).toEqual([
      { type: 'progress', run_id: 'run-1', text: 'Running `git status`…' },
    ])
  })

  it('leaves a row without a progress block untouched', () => {
    const blocks = [{ type: 'markdown' as const, text: 'hi' }]
    const rows = mergeTimeline(
      [serverItem({ id: 'msg-1', run_id: 'run-1', blocks })],
      [],
      {},
      { 'run-1': runProgress({}) },
    )
    expect(rows[0].blocks).toBe(blocks)
  })

  it('renders a run whose progress row has not arrived yet', () => {
    const rows = mergeTimeline([], [], {}, { 'run-1': runProgress({}) })
    expect(rows[0].key).toBe('msg-p1')
    expect(rows[0].status).toBe('streaming')
    expect(rows[0].runId).toBe('run-1')
    expect(rows[0].authorAgentId).toBe('ag-1')
    expect(rows[0].blocks).toEqual([
      { type: 'progress', run_id: 'run-1', text: 'Running `git status`…' },
    ])
  })

  it('never doubles a progress row the server page already carries', () => {
    const rows = mergeTimeline(
      [
        serverItem({
          id: 'msg-p1',
          author_kind: 'agent',
          run_id: 'run-1',
          blocks: [{ type: 'progress', run_id: 'run-1', text: 'Done' }],
        }),
      ],
      [],
      {},
      { 'run-1': runProgress({}) },
    )
    expect(rows.map((r) => r.key)).toEqual(['msg-p1'])
  })
})

describe('threadScope', () => {
  it('is the channel id at the top level', () => {
    expect(threadScope('ch-1')).toBe('ch-1')
    expect(threadScope('ch-1', null)).toBe('ch-1')
  })

  it('is channel/root inside a thread', () => {
    expect(threadScope('ch-1', 'msg-root')).toBe('ch-1/msg-root')
  })
})

describe('formatClock', () => {
  it('reads the hour and the minute, never the seconds', () => {
    const at = new Date(2026, 8, 5, 15, 6, 43).getTime()
    expect(formatClock(at)).toBe(
      new Date(at).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' }),
    )
    expect(formatClock(at)).not.toContain('43')
  })
})

describe('plainText', () => {
  it('reads what the writer wrote, not the Markdown that carries it', () => {
    expect(
      plainText('Yes, **Cascade Link, Fibre 900** on the yearly term. Order `BM92PT`.'),
    ).toBe('Yes, Cascade Link, Fibre 900 on the yearly term. Order BM92PT.')
  })

  it('keeps the words of a link and drops its address', () => {
    expect(plainText('See [the quote](https://example.com/quote).')).toBe(
      'See the quote.',
    )
  })

  it('flattens a heading, a list and a quote into one line', () => {
    expect(plainText('# Trips\n\n- Austin\n- San Jose\n\n> past')).toBe(
      'Trips Austin San Jose past',
    )
  })
})
