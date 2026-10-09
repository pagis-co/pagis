// The sprite profile at phone width: the badges of its identity, the
// desk card, and the rows that open Access, Memory and Work.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory } from '@tanstack/react-router'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'
import { App } from '../../App'
import type { ApiClient } from '../../api/client'
import { shellResponse } from '../../test/appStub'

const { api } = vi.hoisted(() => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
}))
vi.mock('../../api/client', async () => ({
  ...(await vi.importActual('../../api/client')),
  createApiClient: () => api as unknown as ApiClient,
}))
vi.mock('../../ws/socket', () => ({
  PagisSocket: class {
    start() {}
    stop() {}
    subscribeChannel() {}
    activity() {
      return false
    }
  },
}))

let responses: Record<string, unknown>

beforeEach(() => {
  vi.stubGlobal('matchMedia', (query: string) => ({
    matches: query.includes('max-width'),
    media: query,
    addEventListener() {},
    removeEventListener() {},
  }))
  vi.stubGlobal('fetch', vi.fn(async () => new Response('missing', { status: 404 })))
  responses = {
    '/api/v1/settings/phone-numbers': {
      items: [{ id: 'number-1', agent_id: 'agent-1', e164: '+14155550199' }],
      carrier: null,
    },
    '/api/v1/grants': {
      items: [
        { id: 'grant-1', agent_id: 'agent-1', resource_kind: 'connection', resource_id: 'c-1', capabilities: [] },
        { id: 'grant-2', agent_id: 'agent-1', resource_kind: 'host', resource_id: 'h-1', capabilities: [] },
      ],
    },
    '/api/v1/memory/feed': {
      items: [{ id: 'change-1', message: 'Changed today: Northwind billing', created_at: 1 }],
    },
    '/api/v1/runs': {
      items: [{ id: 'run-1', agent_id: 'agent-1', title: 'Filed the September receipts', created_at: Date.now() }],
    },
  }
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

it('shows the phone number of the sprite in its written form', async () => {
  mount('/sprites/agent-1')
  expect(await screen.findByText('+1 415 555 0199')).not.toBeNull()
  expect(screen.queryByText('+14155550199')).toBeNull()
})

it('sums the access and shows the latest memory change and run', async () => {
  const history = mount('/sprites/agent-1')
  expect(await screen.findByText('1 account and 1 computer')).not.toBeNull()
  expect(await screen.findByText('Changed today: Northwind billing')).not.toBeNull()
  expect(await screen.findByText(/^Filed the September receipts · /)).not.toBeNull()
  fireEvent.click(screen.getByRole('button', { name: /^Access/ }))
  await waitFor(() => expect(history.location.pathname).toBe('/sprites/agent-1/access'))
})

it('opens the desk from the desk card, and puts no control inside it when the live screen fails', async () => {
  responses['/api/v1/agents/{agent_id}/computer'] = { state: 'awake', percent: null, holder: 'agent' }
  // jsdom has no WebRTC, so the live screen fails as it does when the
  // relay refuses it.
  const history = mount('/sprites/agent-1')
  expect(await screen.findByText('Live screen unavailable.')).not.toBeNull()
  expect(document.querySelector('button button')).toBeNull()
  fireEvent.click(screen.getByRole('button', { name: /Sage's desk/ }))
  await waitFor(() => expect(history.location.pathname).toBe('/sprites/agent-1/desk'))
})
