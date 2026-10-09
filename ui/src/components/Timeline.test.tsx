// The Thread: one name line for a run of messages by one author,
// the work record in place of the "Done" row, the Working row in place
// of "Thinking…", the quiet Stopped line, the Today and New dividers,
// the search of the conversation, and the pill that returns to the end
// once the reader has scrolled away.
//
// Virtuoso measures a viewport that jsdom does not lay out, so the list
// is replaced by a stub that renders every item and hands the test the
// two seams the timeline uses: the at-bottom report and `scrollToIndex`.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, screen, waitFor } from '@testing-library/react'
import { forwardRef, useImperativeHandle, type ReactNode } from 'react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { renderInRouter } from '../test/router'
import { shellResponse } from '../test/appStub'
import { useComposerDraft } from '../state/composerDraft'
import {
  useCallInspector,
  useDeskFocus,
  useMailInspector,
  useThreadSearch,
} from '../state/stores'
import { Timeline } from './Timeline'

const list = vi.hoisted(() => ({
  atBottomStateChange: (_atBottom: boolean) => {},
  scrollToIndex: vi.fn(),
}))

vi.mock('react-virtuoso', () => ({
  Virtuoso: forwardRef(function VirtuosoStub(
    {
      data,
      itemContent,
      computeItemKey,
      atBottomStateChange,
    }: {
      data: unknown[]
      itemContent: (index: number, item: unknown) => ReactNode
      computeItemKey: (index: number, item: unknown) => string
      atBottomStateChange: (atBottom: boolean) => void
    },
    ref,
  ) {
    list.atBottomStateChange = atBottomStateChange
    useImperativeHandle(ref, () => ({ scrollToIndex: list.scrollToIndex }))
    return (
      <div>
        {data.map((item, index) => (
          <div key={computeItemKey(index, item)}>{itemContent(index, item)}</div>
        ))}
      </div>
    )
  }),
}))

const at = Date.parse('2026-09-05T10:00:00Z')

function message(id: string, overrides: Record<string, unknown> = {}) {
  return {
    id,
    channel_id: 'ch-1',
    author_kind: 'agent',
    author_agent_id: 'agent-1',
    status: 'complete',
    blocks: [{ type: 'markdown', text: `line ${id}` }],
    text_content: `line ${id}`,
    created_at: at,
    reply_count: 0,
    ...overrides,
  }
}

/** The channel as the API serves it: newest first. */
const items = [
  message('m3', { created_at: at + 2000 }),
  message('m2', { created_at: at + 1000, run_id: 'run-1' }),
  message('p1', {
    created_at: at,
    run_id: 'run-1',
    blocks: [{ type: 'progress', run_id: 'run-1', text: 'Done' }],
    text_content: 'Done',
  }),
  message('m1', { created_at: at - 1000 }),
]

/** The steps of `run-1`: one step that took a screen. */
const runSteps = {
  run_id: 'run-1',
  state: 'completed',
  worked_ms: 9_000,
  stopped: { by: 'user', after_ms: 12_000 },
  failure: null,
  steps: [
    {
      index: 1,
      label: 'computer',
      kind: 'desk',
      started_at: 0,
      ended_at: 9_000,
      duration_ms: 9_000,
      screenshot_id: 'art-1',
      live: false,
    },
  ],
}

function stubApi(timelineItems = items, unavailable = false) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/channels/{channel_id}/messages') {
        if (unavailable) throw new Error('Unavailable')
        return { data: { items: timelineItems } }
      }
      if (path === '/api/v1/runs/{run_id}/steps') {
        return { data: runSteps }
      }
      if (path === '/api/v1/agents') {
        return { data: { items: [{ id: 'agent-1', name: 'Sage' }] } }
      }
      return shellResponse(path)
    }),
  }
}

function mount(
  timelineItems = items,
  unavailable = false,
  onOpenDesk = vi.fn(),
  onOpenThread = vi.fn(),
) {
  const api = stubApi(timelineItems, unavailable)
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  // A row with a run links to it through the router.
  renderInRouter(
    <QueryClientProvider client={queryClient}>
      <Timeline
        api={api as unknown as ApiClient}
        channelId="ch-1"
        onOpenThread={onOpenThread}
        onOpenChannel={() => {}}
        onOpenDesk={onOpenDesk}
      />
    </QueryClientProvider>,
  )
  return { onOpenDesk, onOpenThread }
}

describe('Timeline', () => {
  it('reports a failed history read instead of an empty conversation', async () => {
    mount([], true)
    expect(await screen.findByText('Could not load this conversation')).toBeTruthy()
    expect(screen.queryByText('No messages yet. Say hello.')).toBeNull()
  })

  it('keeps a system mail record and its inspector available', async () => {
    const block = {
      type: 'mail', direction: 'inbound', mailbox: 'sage@example.com',
      message_id: 'INBOX:12', counterpart: 'team@example.com',
      subject: 'Project update', trust_tier: 'unknown',
    }
    mount([message('mail-message', {
      author_kind: 'system', author_agent_id: null,
      blocks: [block], text_content: '[mail]',
    })])
    fireEvent.click(await screen.findByTestId('mail-strip'))
    expect(useMailInspector.getState().mail).toEqual(block)
    expect(screen.queryByText('[mail]')).toBeNull()
  })

  it('keeps a system-authored call interactive in the conversation', async () => {
    useCallInspector.getState().close()
    mount([message('call-message', {
      author_kind: 'system',
      author_agent_id: null,
      blocks: [{ type: 'call', call_id: 'call_1' }],
      text_content: '[call]',
    })])

    fireEvent.click(await screen.findByTestId('call-strip'))
    expect(useCallInspector.getState().callId).toBe('call_1')
    expect(screen.queryByText('[call]')).toBeNull()
  })

  // A daemon-made block at the top level is the root of its Thread
  // (ADR-0033, ADR-0022). It shows the replies chip and "Reply in
  // thread", as every other root does.
  it.each([
    [
      'coding session',
      {
        type: 'coding_session',
        coding_session_id: 'session-1',
        harness: 'Codex',
        machine: 'Ada’s laptop',
        directory: '/Users/ada/src/app',
        title: 'Update the readme',
      },
    ],
    ['call', { type: 'call', call_id: 'call_1' }],
  ])('opens the Thread of a %s block from its replies chip', async (_kind, block) => {
    const { onOpenThread } = mount(
      [
        message('block-message', {
          author_kind: 'system',
          author_agent_id: null,
          blocks: [block],
          text_content: '[block]',
          reply_count: 19,
          last_reply_at: at + 1000,
          reply_authors: [{ author_kind: 'agent', author_agent_id: 'agent-1' }],
        }),
      ],
      false,
      vi.fn(),
      vi.fn(),
    )

    fireEvent.click(await screen.findByRole('button', { name: /19 replies/ }))
    expect(onOpenThread).toHaveBeenCalledWith('block-message')

    onOpenThread.mockClear()
    fireEvent.click(screen.getByRole('button', { name: 'Reply in thread' }))
    expect(onOpenThread).toHaveBeenCalledWith('block-message')
  })

  it('gives a run of messages by one author one name line', async () => {
    mount()

    await screen.findByText('line m1')
    expect(screen.getAllByText('Sage')).toHaveLength(1)
    expect(screen.getAllByTestId('message-row')).toHaveLength(3)
  })

  it('shows the work record in place of the "Done" row', async () => {
    mount()

    await screen.findByText('line m1')
    expect(screen.queryByText('Done')).toBeNull()
    // The run is folded under the reply it produced.
    expect(screen.getByTestId('work-record')).toBeTruthy()
  })

  it('scrolls the Desk panel to the screen a step took', async () => {
    useDeskFocus.getState().clear()
    const { onOpenDesk } = mount()

    fireEvent.click(await screen.findByText(/1 step/))
    fireEvent.click(screen.getByText('see'))

    expect(useDeskFocus.getState().screenshotId).toBe('art-1')
    expect(onOpenDesk).toHaveBeenCalled()
  })

  it('puts the Working row at the end in place of "Thinking…"', async () => {
    mount([
      message('p2', {
        created_at: at + 3000,
        status: 'streaming',
        run_id: 'run-2',
        blocks: [{ type: 'progress', run_id: 'run-2', text: 'Thinking…' }],
        text_content: 'Thinking…',
      }),
      ...items,
    ])

    const working = await screen.findByTestId('working-row')
    expect(screen.getByText('Sage is working')).toBeTruthy()
    expect(screen.queryAllByTestId('message-row')).toHaveLength(3)
    // The live row is the last item of the column.
    const rows = screen.getAllByTestId(/message-row|working-row/)
    expect(rows[rows.length - 1]).toBe(working)
  })

  it('says a stopped Run in a quiet line, and asks again from the composer', async () => {
    useComposerDraft.setState({ byScope: {} })
    mount([
      message('p9', {
        created_at: at + 3000,
        run_id: 'run-1',
        blocks: [{ type: 'progress', run_id: 'run-1', text: 'Stopped' }],
        text_content: 'Stopped',
      }),
      message('u1', {
        created_at: at + 2500,
        author_kind: 'user',
        author_agent_id: null,
        text_content: 'Book the Paris one',
        blocks: [{ type: 'markdown', text: 'Book the Paris one' }],
      }),
    ])

    expect(await screen.findByText('Stopped')).toBeTruthy()
    fireEvent.click(await screen.findByText('Ask again'))

    expect(useComposerDraft.getState().byScope['ch-1']).toBe('Book the Paris one')
  })

  it('draws the Today divider', async () => {
    const today = Date.now() - 60_000
    mount([message('m1', { created_at: today })])

    expect((await screen.findByTestId('day-divider')).textContent).toBe('Today')
  })

  it('keeps only the messages a search of the conversation finds', async () => {
    useThreadSearch.getState().set('ch-1', 'M2')
    mount()

    expect(await screen.findByText('line m2')).toBeTruthy()
    expect(screen.queryByText('line m1')).toBeNull()
    expect(screen.queryByTestId('day-divider')).toBeNull()

    act(() => useThreadSearch.getState().set('ch-1', 'nothing here'))
    expect(await screen.findByText('No message matches')).toBeTruthy()
    act(() => useThreadSearch.getState().set('ch-1', ''))
  })

  it('offers the jump pill off the bottom, and returns to the end', async () => {
    mount()
    await screen.findByText('line m1')

    expect(screen.queryByText('Jump to latest')).toBeNull()

    act(() => list.atBottomStateChange(false))
    fireEvent.click(await screen.findByText('Jump to latest'))

    expect(list.scrollToIndex).toHaveBeenCalledWith(
      expect.objectContaining({ index: 3, align: 'end' }),
    )

    act(() => list.atBottomStateChange(true))
    await waitFor(() => expect(screen.queryByText('Jump to latest')).toBeNull())
  })
})
