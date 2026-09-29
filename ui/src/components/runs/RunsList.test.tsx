// `/runs` reads as a record: the day groups the rows, a failed
// run says why in plain words, and a filter chip carries the count it
// would give — which moves when another chip changes.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, RunDto } from '../../api/client'
import { RunsList } from './RunsList'

const NOW = Date.now()

function run(fields: Partial<RunDto>): RunDto {
  return {
    id: 'run-1',
    agent_id: 'agent-1',
    channel_id: 'channel-1',
    root_message_id: null,
    trigger_kind: 'message',
    trigger_ref: 'message-1',
    hop_count: 0,
    state: 'completed',
    error: null,
    started_at: NOW,
    ended_at: NOW + 600,
    created_at: NOW,
    duration_ms: 600,
    ...fields,
  }
}

const runs = [
  run({ id: 'a', state: 'completed' }),
  run({
    id: 'b',
    state: 'failed',
    failure_kind: 'tool_failed',
    error: 'unrelated detail',
  }),
  run({ id: 'c', agent_id: 'agent-2', state: 'failed', failure_kind: 'model_failed', error: 'unrelated detail' }),
]

function stubApi() {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/agents') {
        return {
          data: {
            items: [
              { id: 'agent-1', name: 'Sage', job: 'assistant', status: 'active' },
              { id: 'agent-2', name: 'Nova', job: 'assistant', status: 'active' },
            ],
          },
        }
      }
      if (path === '/api/v1/channels') {
        return { data: { items: [{ id: 'channel-1', title: 'Sage', kind: 'dm' }] } }
      }
      if (path === '/api/v1/runs') return { data: { items: runs } }
      return { data: { items: [] } }
    }),
  } as unknown as ApiClient
}

function mount(onOpenRun = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <RunsList api={stubApi()} onOpenRun={onOpenRun} onOpenNav={vi.fn()} />
    </QueryClientProvider>,
  )
  return onOpenRun
}

/** The chip of one dimension, by the word it carries. */
function chip(legend: string, label: string | RegExp): HTMLElement {
  const group = screen.getByRole('group', { name: legend })
  return within(group).getByRole('button', { name: new RegExp(label, 'i') })
}

describe('the runs record', () => {
  it('groups the rows by day and names the trigger', async () => {
    mount()

    expect(await screen.findByRole('region', { name: 'Today' })).toBeTruthy()
    expect(screen.getAllByText('A message in Sage').length).toBe(3)
  })

  it('says why a run failed in plain words, not as a JSON blob', async () => {
    mount()

    expect(
      await screen.findByText('Ended because a tool failed'),
    ).toBeTruthy()
    expect(screen.getByText('Ended because the model request failed')).toBeTruthy()
  })

  it('opens the run it is clicked on', async () => {
    const onOpenRun = mount()

    const today = await screen.findByRole('region', { name: 'Today' })
    fireEvent.click(within(today).getAllByRole('button')[0])
    expect(onOpenRun).toHaveBeenCalledWith('a')
  })

  it('moves a chip count when another filter changes', async () => {
    mount()

    await waitFor(() => expect(chip('State', 'Failed').textContent).toBe('Failed2'))

    fireEvent.click(chip('Sprite', 'Sage'))

    await waitFor(() => expect(chip('State', 'Failed').textContent).toBe('Failed1'))
    // The chip drops its own filter, so it still counts what clicking
    // it would give.
    expect(chip('State', 'Done').textContent).toBe('Done1')
    expect(screen.queryByText('Ended because the model request failed')).toBeNull()
  })
})
