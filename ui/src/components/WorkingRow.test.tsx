// The Working row: the Agent, the step it is on, the time it
// has worked, the desk and Stop.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import type { TimelineRow } from '../timeline'
import { WorkingRow, formatTimer } from './WorkingRow'

function row(text = 'Searching mail for itineraries'): TimelineRow {
  return {
    key: 'p1',
    kind: 'message',
    authorKind: 'agent',
    authorAgentId: 'ag-1',
    createdAt: Date.now() - 42_000,
    sendState: 'sent',
    status: 'streaming',
    runId: 'run-1',
    blocks: [{ type: 'progress', run_id: 'run-1', text }],
    text,
    completedAt: null,
    replyCount: 0,
    lastReplyAt: null,
    replyAuthors: [],
  }
}

const steps = {
  run_id: 'run-1',
  state: 'running',
  worked_ms: null,
  stopped: null,
  failure: null,
  steps: [
    {
      index: 1,
      label: 'woke the computer',
      kind: 'desk',
      started_at: 0,
      ended_at: 1,
      duration_ms: 1,
      screenshot_id: null,
      live: false,
    },
    {
      index: 2,
      label: 'mail search',
      kind: 'mail',
      started_at: 1,
      ended_at: null,
      duration_ms: null,
      screenshot_id: null,
      live: true,
    },
  ],
}

function mount(
  progress = row(),
  cancel: () => Promise<unknown> = async () => ({
    data: { run_id: 'run-1', outcome: 'canceling', state: null },
  }),
) {
  const onOpenDesk = vi.fn()
  const api = { GET: vi.fn(async () => ({ data: steps })), POST: vi.fn(cancel) }
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <WorkingRow
        api={api as unknown as ApiClient}
        row={progress}
        agentName="Sage"
        onOpenDesk={onOpenDesk}
      />
    </QueryClientProvider>,
  )
  return { api, onOpenDesk }
}

describe('formatTimer', () => {
  it('reads the minutes and the seconds', () => {
    expect(formatTimer(42_000)).toBe('0:42')
    expect(formatTimer(605_000)).toBe('10:05')
  })
})

describe('WorkingRow', () => {
  it('names the Agent, the step and the time it has worked', async () => {
    mount()

    expect(screen.getByText('Sage is working')).toBeTruthy()
    expect(screen.getByText('· Searching mail for itineraries')).toBeTruthy()
    expect(await screen.findByText('step 2 of the Run · 0:42')).toBeTruthy()
  })

  it('opens the desk and stops the Run', async () => {
    const { api, onOpenDesk } = mount()

    fireEvent.click(screen.getByLabelText('Open the desk'))
    expect(onOpenDesk).toHaveBeenCalled()

    fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/runs/{run_id}/cancel', {
        params: { path: { run_id: 'run-1' } },
      }),
    )
  })

  it('says it is stopping until the Run ends, and takes no second click', async () => {
    const { api } = mount()

    fireEvent.click(screen.getByRole('button', { name: 'Stop' }))

    const stopping = await screen.findByRole('button', { name: 'Stopping…' })
    expect((stopping as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(stopping)
    expect(api.POST).toHaveBeenCalledTimes(1)
  })

  it('offers Stop again when the stop request fails', async () => {
    let answer: (value: unknown) => void = () => undefined
    mount(row(), () => new Promise((resolve) => (answer = resolve)))

    fireEvent.click(screen.getByRole('button', { name: 'Stop' }))
    await screen.findByRole('button', { name: 'Stopping…' })
    answer({ error: { error: { code: 'internal', message: 'down' } } })

    const stop = await screen.findByRole('button', { name: 'Stop' })
    expect((stop as HTMLButtonElement).disabled).toBe(false)
  })

  it('shows reflection without the working signals', () => {
    mount(row('Updating memory'))

    const status = screen.getByTestId('reflecting-row')
    expect(status.textContent).toContain('Updating memory')
    expect(screen.queryByText('Sage is working')).toBeNull()
    expect(screen.queryByLabelText('Open the desk')).toBeNull()
    expect(status.querySelector('[data-presence="none"]')).toBeTruthy()
    expect(screen.getByText('Stop')).toBeTruthy()
  })
})
