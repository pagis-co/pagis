// The places under You at phone width: the Memory page list, the
// Automations lists and the Software packages.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory } from '@tanstack/react-router'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'
import { App } from '../App'
import type { ApiClient } from '../api/client'
import { shellResponse } from '../test/appStub'
import { formatClock } from '../timeline'

const { api } = vi.hoisted(() => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
}))
vi.mock('../api/client', async () => ({
  ...(await vi.importActual('../api/client')),
  createApiClient: () => api as unknown as ApiClient,
}))
vi.mock('../ws/socket', () => ({
  PagisSocket: class {
    start() {}
    stop() {}
    subscribeChannel() {}
    activity() {
      return false
    }
  },
}))

const DAY = 86_400_000

const schedule = (overrides: Record<string, unknown>) => ({
  id: 'sc1',
  workspace_id: 'workspace-1',
  agent_id: 'agent-1',
  name: 'Morning brief',
  instruction: 'Summarize the inbox',
  channel_id: 'channel-1',
  kind: 'cron',
  cron_expression: '30 7 * * *',
  timezone: 'UTC',
  scheduled_at: 1,
  next_due_at: null,
  state: 'active',
  revision: 1,
  creator: 'user',
  created_at: 1,
  updated_at: 1,
  ...overrides,
})

let responses: Record<string, unknown>

beforeEach(() => {
  vi.stubGlobal('matchMedia', (query: string) => ({
    matches: query.includes('max-width'),
    media: query,
    addEventListener() {},
    removeEventListener() {},
  }))
  vi.stubGlobal('fetch', vi.fn(async () => new Response('missing', { status: 404 })))
  responses = {}
  api.GET.mockReset()
  api.GET.mockImplementation(async (path: string) =>
    path in responses ? { data: responses[path] } : shellResponse(path),
  )
})

function mount(path: string) {
  const history = createMemoryHistory({ initialEntries: [path] })
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <App history={history} />
    </QueryClientProvider>,
  )
  return history
}

it('lists Schedules with a short next time, and a paused one once', async () => {
  const tomorrow = new Date(Date.now() + DAY)
  tomorrow.setHours(7, 30, 0, 0)
  responses['/api/v1/schedules'] = {
    items: [
      schedule({ next_due_at: tomorrow.getTime() }),
      schedule({ id: 'sc2', name: 'Supplier prices', state: 'paused' }),
    ],
  }
  mount('/automations')
  expect(
    await screen.findByText(`Next: tomorrow ${formatClock(tomorrow.getTime())} · Sage`),
  ).not.toBeNull()
  const paused = screen.getByRole('button', { name: /Supplier prices/ })
  expect(paused.textContent?.match(/Paused/g)).toHaveLength(1)
})

it('names the event of a subscription in words', async () => {
  responses['/api/v1/event-subscriptions'] = {
    items: [
      {
        id: 'es1',
        workspace_id: 'workspace-1',
        agent_id: 'agent-1',
        connection_id: 'cn1',
        event_kind: 'mail.message_received',
        source_version: '1',
        name: 'Invoice watch',
        instruction: 'File the invoice',
        channel_id: 'channel-1',
        filter: {},
        creator: 'user',
        state: 'paused',
        revision: 1,
        created_at: 1,
        updated_at: 1,
      },
    ],
  }
  mount('/automations')
  expect(await screen.findByText('New mail · Sage')).not.toBeNull()
  expect(screen.queryByText(/mail\.message_received/)).toBeNull()
  expect(screen.getByText('Paused')).not.toBeNull()
})

it('says that nothing waits once, and opens a Schedule', async () => {
  const row = schedule({ next_due_at: Date.now() + DAY })
  responses['/api/v1/schedules'] = { items: [row] }
  responses['/api/v1/schedules/{schedule_id}'] = row
  responses['/api/v1/schedules/{schedule_id}/occurrences'] = { items: [], next_cursor: null }
  responses['/api/v1/schedules/{schedule_id}/wakeups'] = { items: [], next_cursor: null }
  const history = mount('/automations')
  expect(await screen.findAllByText('Nothing waits for you.')).toHaveLength(1)
  fireEvent.click(await screen.findByRole('button', { name: /Morning brief/ }))
  await waitFor(() =>
    expect(decodeURIComponent(history.location.search)).toContain('"id":"sc1"'),
  )
})

it('lists the Software packages with their author and tools', async () => {
  responses['/api/v1/software'] = {
    items: [
      {
        name: 'invoice-tools',
        latest_version: '1.2.0',
        author_agent_id: 'agent-1',
        author_name: 'Sage',
        keywords: [],
        tool_count: 1,
        open_contributions: 0,
      },
    ],
  }
  mount('/software')
  expect(await screen.findByText('invoice-tools')).not.toBeNull()
  expect(screen.getByText('Sage · 1 tool · version 1.2.0')).not.toBeNull()
})

it('lists the Memory pages of the shared scope', async () => {
  responses['/api/v1/memory/pages'] = {
    pages: [
      {
        path: 'household.md',
        scope: 'shared',
        title: 'Household',
        excerpt: 'The boiler is serviced in March.',
        changed_at: Date.now(),
        changed_by: 'Sage',
        changed_by_agent_id: 'agent-1',
      },
    ],
    next: null,
    total: 1,
  }
  responses['/api/v1/memory/file'] = {
    path: 'household.md',
    scope: 'shared',
    content: '# Household\n\nThe boiler is serviced in March.',
    content_kind: 'markdown',
    revision: 'r1',
    source_scoped: false,
  }
  const history = mount('/memory')
  expect(await screen.findByText('The boiler is serviced in March.')).not.toBeNull()
  fireEvent.click(screen.getByText('Household'))
  await waitFor(() => expect(history.location.search).toContain('path=household.md'))
})
