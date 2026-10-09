// The Timeline names the Agent whose Desk a row opens. A screen that a
// step took and the live tile of the Working row each belong to the
// Agent of their row, so the route opens that Agent's Desk: the panel
// on the desktop, the live screen on the phone.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, screen } from '@testing-library/react'
import { forwardRef, type ReactNode } from 'react'
import { expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { shellResponse } from '../test/appStub'
import { renderInRouter } from '../test/router'
import { Timeline } from './Timeline'

vi.mock('react-virtuoso', () => ({
  Virtuoso: forwardRef(function VirtuosoStub({
    data,
    itemContent,
    computeItemKey,
  }: {
    data: unknown[]
    itemContent: (index: number, item: unknown) => ReactNode
    computeItemKey: (index: number, item: unknown) => string
  }) {
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
    author_agent_id: 'agent-2',
    status: 'complete',
    blocks: [{ type: 'markdown', text: `line ${id}` }],
    text_content: `line ${id}`,
    created_at: at,
    reply_count: 0,
    ...overrides,
  }
}

function mount(items: unknown[]) {
  const api = {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/channels/{channel_id}/messages') return { data: { items } }
      if (path === '/api/v1/runs/{run_id}/steps') {
        return {
          data: {
            run_id: 'run-1',
            state: 'completed',
            worked_ms: 9_000,
            stopped: null,
            failure: null,
            steps: [{ index: 1, label: 'computer', kind: 'desk', started_at: 0, ended_at: 9_000, duration_ms: 9_000, screenshot_id: 'art-1', live: false }],
          },
        }
      }
      if (path === '/api/v1/agents') return { data: { items: [{ id: 'agent-2', name: 'Pebble' }] } }
      return shellResponse(path)
    }),
  }
  const onOpenDesk = vi.fn()
  renderInRouter(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <Timeline
        api={api as unknown as ApiClient}
        channelId="ch-1"
        onOpenThread={() => {}}
        onOpenChannel={() => {}}
        onOpenDesk={onOpenDesk}
      />
    </QueryClientProvider>,
  )
  return onOpenDesk
}

it('opens the Desk of the Agent whose step took the screen', async () => {
  const onOpenDesk = mount([
    message('m2', { created_at: at + 1000, run_id: 'run-1' }),
    message('p1', { run_id: 'run-1', blocks: [{ type: 'progress', run_id: 'run-1', text: 'Done' }], text_content: 'Done' }),
  ])

  fireEvent.click(await screen.findByText(/1 step/))
  fireEvent.click(screen.getByText('see'))

  expect(onOpenDesk).toHaveBeenCalledTimes(1)
  expect(onOpenDesk).toHaveBeenCalledWith('agent-2')
})

it('opens the Desk of the working Agent from its live tile', async () => {
  const onOpenDesk = mount([
    message('p2', {
      status: 'streaming',
      run_id: 'run-2',
      blocks: [{ type: 'progress', run_id: 'run-2', text: 'Thinking…' }],
      text_content: 'Thinking…',
    }),
  ])

  fireEvent.click(await screen.findByRole('button', { name: 'Open the desk' }))

  expect(onOpenDesk).toHaveBeenCalledTimes(1)
  expect(onOpenDesk).toHaveBeenCalledWith('agent-2')
})
