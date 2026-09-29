// The work record: one line that says how long the Run worked,
// how many steps it took and what it touched. The line opens to the
// steps, and a step with a screen offers the tile that scrolls the
// Desk panel to that screenshot.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import type { WorkSummary } from '../timeline'
import { WorkRecord } from './WorkRecord'

const work: WorkSummary = {
  runId: 'run-1',
  startedAt: 0,
  endedAt: 51_000,
  outcome: 'Done',
}

const steps = {
  run_id: 'run-1',
  state: 'completed',
  worked_ms: 65_000,
  stopped: null,
  failure: null,
  steps: [
    {
      index: 1,
      label: 'Searched your mail',
      kind: 'mail',
      started_at: 0,
      ended_at: 9_000,
      duration_ms: 9_000,
      screenshot_id: null,
      live: false,
    },
    {
      index: 2,
      label: 'Read your calendar',
      kind: 'calendar',
      started_at: 9_000,
      ended_at: 65_000,
      duration_ms: 56_000,
      screenshot_id: 'art-9',
      live: false,
    },
  ],
}

function mount(onShowScreenshot = vi.fn()) {
  const api = { GET: vi.fn(async () => ({ data: steps })) }
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <WorkRecord
        api={api as unknown as ApiClient}
        work={work}
        onShowScreenshot={onShowScreenshot}
      />
    </QueryClientProvider>,
  )
  return { api, onShowScreenshot }
}

describe('WorkRecord', () => {
  it('states the duration, the steps and what the Run touched', async () => {
    mount()

    const line = await screen.findByText(
      'Worked 1 min 5 s · 2 steps · mail, calendar',
    )
    expect(line.closest('button')?.getAttribute('aria-expanded')).toBe('false')
    expect(screen.queryByText('Searched your mail')).toBeNull()
  })

  it('opens to the steps', async () => {
    mount()

    fireEvent.click(await screen.findByText(/2 steps/))

    expect(screen.getByText('Searched your mail')).toBeTruthy()
    expect(screen.getByText('9 s')).toBeTruthy()
  })

  it('scrolls the Desk panel to the screen one step took', async () => {
    const { onShowScreenshot } = mount()

    fireEvent.click(await screen.findByText(/2 steps/))
    fireEvent.click(screen.getByText('see'))

    expect(onShowScreenshot).toHaveBeenCalledWith('art-9')
  })
})
