// `/runs/:runId` reads as a timeline: the trigger opens it, each
// tool call is a step card of one line in and one line out, the raw
// event stays one disclosure away, and the footer carries the tokens
// and the time.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { RunTimeline } from './RunTimeline'

const run = {
  id: 'run-1',
  agent_id: 'agent-1',
  channel_id: 'channel-1',
  root_message_id: null,
  trigger_kind: 'message',
  trigger_ref: 'message-1',
  hop_count: 0,
  state: 'failed',
  failure_kind: 'tool_failed',
    error: 'unrelated detail',
  started_at: 1_000,
  ended_at: 3_000,
  created_at: 1_000,
  duration_ms: 2_000,
}

const events = [
  {
    id: 'e1',
    event_type: 'tool.called',
    agent_id: 'agent-1',
    channel_id: 'channel-1',
    created_at: 1_000,
    payload: {
      name: 'computer',
      arguments: JSON.stringify({
        actions: [{ type: 'click' }],
        call_id: 'call_1',
      }),
    },
  },
  {
    id: 'e2',
    event_type: 'tool.completed',
    agent_id: 'agent-1',
    channel_id: 'channel-1',
    created_at: 1_500,
    payload: { name: 'computer', ok: true, duration_ms: 480 },
  },
  {
    id: 'e3',
    event_type: 'memory.review_pending',
    agent_id: 'agent-1',
    channel_id: 'channel-1',
    created_at: 1_600,
    payload: {
      pending_id: 'pending-1',
      subject: 'Friday quote',
      urgency: 'normal',
      eligible_at: 301_600,
      maximum_due_at: 1_801_600,
    },
  },
  {
    id: 'e4',
    event_type: 'context.compacted',
    agent_id: 'agent-1',
    channel_id: 'channel-1',
    created_at: 1_700,
    payload: {
      before_estimated_input_tokens: 800,
      after_estimated_input_tokens: 420,
      memory_changes: 0,
      memory_outcome: 'no_change',
    },
  },
  {
    id: 'e5',
    event_type: 'memory.committed',
    agent_id: 'agent-1',
    channel_id: 'channel-1',
    created_at: 1_800,
    payload: {
      sha: 'sha-1',
      message: 'Noted the Friday quote deadline.',
      files: ['private/subjects/quote.md'],
      phase: 'compaction',
      source_range: {
        after_exclusive: 'message-1',
        through_inclusive: 'message-9',
      },
    },
  },
]

function stubApi(runValue = run, eventValues = events, memoryItems: unknown[] | null = null) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/agents') {
        return {
          data: { items: [{ id: 'agent-1', name: 'Sage', job: 'a', status: 'active' }] },
        }
      }
      if (path === '/api/v1/channels') {
        return { data: { items: [{ id: 'channel-1', title: 'Sage', kind: 'dm' }] } }
      }
      if (path === '/api/v1/runs/{run_id}/events') {
        return {
          data: { run: runValue, usage: { input_tokens: 40, output_tokens: 12 }, events: eventValues },
        }
      }
      if (path === '/api/v1/memory/feed') {
        return {
          data: {
            revision: 'sha-1',
            items: memoryItems ?? [
              {
                id: 'feed-1',
                kind: 'committed',
                sha: 'sha-1',
                agent_id: 'agent-1',
                agent_name: 'Sage',
                scopes: ['private'],
                files: ['private/subjects/quote.md'],
                message: 'Noted the Friday quote deadline.',
                run_id: 'run-1',
                message_id: null,
                source_scoped: false,
                reverted_sha: null,
                created_at: 1_800,
                action: null,
              },
            ],
          },
        }
      }
      return { data: { items: [] } }
    }),
    POST: vi.fn(async () => ({ data: { sha: 'revert-1' } })),
  } as unknown as ApiClient
}

function mount(api = stubApi()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const onBack = vi.fn()
  render(
    <QueryClientProvider client={client}>
      <RunTimeline api={api} runId="run-1" onBack={onBack} />
    </QueryClientProvider>,
  )
  return onBack
}

describe('the run timeline', () => {
  it('opens with the trigger and the failure in plain words', async () => {
    mount()

    expect(await screen.findByText('Started by a message in Sage')).toBeTruthy()
    expect(within(screen.getByRole('contentinfo')).getByText('Ended because a tool failed')).toBeTruthy()
  })

  it('summarises a step to one line each way', async () => {
    mount()

    const step = await screen.findByTestId('run-step')
    expect(within(step).getByText('computer')).toBeTruthy()
    expect(within(step).getByText('actions: click')).toBeTruthy()
    expect(within(step).getByText('Done')).toBeTruthy()
    expect(within(step).getByText('480 ms')).toBeTruthy()
  })

  it('holds the raw event behind a disclosure', async () => {
    mount()

    const step = await screen.findByTestId('run-step')
    const toggle = within(step).getByRole('button', { name: /show raw event/i })
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    expect(within(step).queryByText(/call_1/)).toBeNull()

    fireEvent.click(toggle)

    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    expect(within(step).getByText(/call_1/)).toBeTruthy()
  })

  it('carries the tokens and the time in the footer', async () => {
    mount()

    expect(await screen.findByText('40 tokens in · 12 tokens out')).toBeTruthy()
    expect(screen.getByText('2 s')).toBeTruthy()
  })

  it('separates deferred review, reply compaction and committed memory', async () => {
    mount()

    expect(await screen.findByText('Memory review queued')).toBeTruthy()
    expect(screen.getByText('Friday quote')).toBeTruthy()
    expect(screen.getByText('Conversation prepared')).toBeTruthy()
    expect(screen.getByText('800 → 420 estimated input tokens')).toBeTruthy()
    const change = screen.getByRole('region', { name: 'Memory changed' })
    expect(within(change).getByText('Noted the Friday quote deadline.')).toBeTruthy()
    expect(within(change).getByText(/after message-1.*message-9/)).toBeTruthy()
    expect(within(change).getByRole('button', { name: 'Revert' })).toBeTruthy()
  })

  it('reverts the exact committed change from the run record', async () => {
    const api = stubApi()
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(
      <QueryClientProvider client={client}>
        <RunTimeline api={api} runId="run-1" onBack={() => {}} />
      </QueryClientProvider>,
    )

    fireEvent.click(await screen.findByRole('button', { name: 'Revert' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/memory/commits/{sha}/revert', {
        params: { path: { sha: 'sha-1' } },
        body: { expected_revision: 'sha-1' },
      }),
    )
  })

  it('does not reveal a memory event that the permitted feed omits', async () => {
    const committedEvent = events.find((event) => event.event_type === 'memory.committed')!
    mount(stubApi(run, [committedEvent], []))

    expect(await screen.findByText('Conversation and memory')).toBeTruthy()
    expect(screen.queryByText('Noted the Friday quote deadline.')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Revert' })).toBeNull()
  })

  it('queues a failed memory review again from the run record', async () => {
    const api = stubApi({ ...run, trigger_kind: 'review' as const }, [])
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Retry memory review' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/runs/{run_id}/retry', {
        params: { path: { run_id: 'run-1' } },
      }),
    )
    expect((await screen.findByRole('status')).textContent).toContain('Memory review queued again')
  })

  it('goes back to the record', async () => {
    const onBack = mount()

    fireEvent.click(await screen.findByRole('button', { name: /all runs/i }))
    expect(onBack).toHaveBeenCalled()
  })
})
