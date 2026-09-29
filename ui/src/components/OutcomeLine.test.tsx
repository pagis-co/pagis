// The quiet lines: a Run the user stopped, and a Run that
// failed. Each one says what happened and offers the act that follows.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { renderInRouter } from '../test/router'
import type { RunOutcome, TimelineRow } from '../timeline'
import { OutcomeLine } from './OutcomeLine'

function row(): TimelineRow {
  return {
    key: 'p1',
    kind: 'message',
    authorKind: 'agent',
    authorAgentId: 'ag-1',
    createdAt: Date.parse('2026-09-05T10:00:00Z'),
    sendState: 'sent',
    status: 'complete',
    runId: 'run-1',
    blocks: [{ type: 'progress', run_id: 'run-1', text: 'Stopped' }],
    text: 'Stopped',
    completedAt: null,
    replyCount: 0,
    lastReplyAt: null,
    replyAuthors: [],
  }
}

function mount(outcome: RunOutcome, data: Record<string, unknown>) {
  const onAskAgain = vi.fn()
  const api = { GET: vi.fn(async () => ({ data })) }
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  renderInRouter(
    <QueryClientProvider client={queryClient}>
      <OutcomeLine
        api={api as unknown as ApiClient}
        row={row()}
        outcome={outcome}
        authorName="Sage"
        onAskAgain={onAskAgain}
      />
    </QueryClientProvider>,
  )
  return { onAskAgain }
}

const base = { run_id: 'run-1', steps: [], worked_ms: 12_000 }

describe('OutcomeLine', () => {
  it('says who stopped the Run, and offers to ask again', async () => {
    const { onAskAgain } = mount('Stopped', {
      ...base,
      state: 'canceled',
      stopped: { by: 'user', after_ms: 12_000 },
      failure: null,
    })

    expect(
      await screen.findByText('You stopped it after 12 s, before it answered.'),
    ).toBeTruthy()
    expect(screen.getByText('Stopped')).toBeTruthy()

    fireEvent.click(screen.getByText('Ask again'))
    expect(onAskAgain).toHaveBeenCalled()
  })

  it('says why a Run failed, and opens it', async () => {
    mount('Failed', {
      ...base,
      state: 'failed',
      stopped: null,
      failure: 'The Southwest site did not load after three tries.',
    })

    expect(
      await screen.findByText('The Southwest site did not load after three tries.'),
    ).toBeTruthy()
    expect(screen.getByText('Failed')).toBeTruthy()
    expect(screen.getByText('Retry')).toBeTruthy()
    expect(screen.getByText('Open the Run').getAttribute('href')).toBe(
      '/runs/run-1',
    )
  })
})
