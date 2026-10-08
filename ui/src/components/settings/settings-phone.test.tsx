// The settings sections at phone width: the short moments and counts
// of each row, and the daemon's own reason when a change fails.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory } from '@tanstack/react-router'
import { fireEvent, render, screen, within } from '@testing-library/react'
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

const DAY = 86_400_000
/** The daemon's ErrorBody, which `unwrap` throws as it comes. */
const refused = { error: { code: 'conflict', message: 'The daemon says no.' } }

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
  api.POST.mockReset()
  api.PUT.mockReset()
  api.DELETE.mockReset()
})

function mount(path: string) {
  const history = createMemoryHistory({ initialEntries: [path] })
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <App history={history} />
    </QueryClientProvider>,
  )
}

it('shows short moments on Sessions, the badge beside the name, and the reason of a failed Remove', async () => {
  const now = Date.now()
  responses['/api/v1/settings/sessions'] = {
    items: [
      {
        id: 's-this',
        client_kind: 'browser',
        client_name: 'Pagis on iPhone',
        current: true,
        created_at: now - 40 * DAY,
        last_used_at: now,
        expires_at: now + 30 * DAY,
      },
      {
        id: 's-ipad',
        client_kind: 'browser',
        client_name: 'Pagis on iPad',
        current: false,
        created_at: now - 40 * DAY,
        last_used_at: now - DAY,
        expires_at: now + 29 * DAY,
      },
    ],
  }
  api.DELETE.mockResolvedValue({ error: refused, response: new Response(null, { status: 409 }) })
  mount('/settings/sessions')
  const name = await screen.findByText('Pagis on iPhone')
  expect(within(name).getByText('This session')).not.toBeNull()
  const [row] = screen.getAllByTestId('session-row')
  expect(row!.textContent).toContain('last used now')
  expect(row!.textContent).not.toContain(new Date(now).toLocaleString())
  fireEvent.click(screen.getByRole('button', { name: 'Remove Pagis on iPad' }))
  expect((await screen.findByRole('alert')).textContent).toBe('The daemon says no.')
})

it('says when a host that is not connected was last seen', async () => {
  responses['/api/v1/hosts'] = {
    items: [
      {
        id: 'host-1',
        name: 'Studio desktop',
        platform: 'linux',
        capabilities: [],
        present: false,
        last_seen_at: Date.now() - DAY,
      },
    ],
  }
  mount('/settings/hosts')
  const row = await screen.findByTestId('host-row')
  expect(row.textContent).toContain('Linux · No commands · last seen yesterday')
})

it('lists the Coding Harnesses under a machine that runs commands, and none under a phone', async () => {
  responses['/api/v1/harnesses'] = {
    items: [{ id: 'claude', name: 'Claude Code', sign_in_methods: [{ method: 'subscription', label: 'Subscription' }] }],
  }
  responses['/api/v1/hosts'] = {
    items: [
      {
        id: 'host-1',
        name: 'Studio desktop',
        platform: 'macos',
        capabilities: ['shell', 'harness:claude'],
        harnesses: [{ id: 'claude', needs_sign_in: true }],
        present: true,
        last_seen_at: Date.now(),
      },
      {
        id: 'host-2',
        name: 'iPhone',
        platform: 'ios',
        capabilities: [],
        harnesses: [],
        present: true,
        last_seen_at: Date.now(),
      },
    ],
  }
  mount('/settings/hosts')
  const claude = await screen.findByRole('group', { name: 'Claude Code' })
  expect(screen.getAllByRole('group', { name: 'Claude Code' })).toHaveLength(1)
  expect(within(claude).getByText('Needs sign-in')).toBeTruthy()
  expect(
    within(claude).getByRole('button', { name: 'Sign in with a subscription' }),
  ).toBeTruthy()
  expect(screen.getByText(/Pagis never sees your password or your key\./)).toBeTruthy()
})

it('shows compact token counts on Usage', async () => {
  const now = Date.now()
  const total = (input: number, output: number, calls: number) => ({
    input_tokens: input,
    output_tokens: output,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    cost_usd: 0.41,
    calls,
  })
  responses['/api/v1/usage'] = {
    from: 0,
    to: 1,
    total: total(2_100_000, 184_000, 1_206),
    monthly_spend_cap_usd: 50,
    runs: [{ run_id: 'run-1', last_at: now, total: total(48_000, 3_100, 9) }],
  }
  mount('/settings/usage')
  expect(
    await screen.findByText('2.1M tokens in · 184K tokens out · 1,206 model calls'),
  ).not.toBeNull()
  expect(screen.getByText('48K in · 3.1K out · 9 calls')).not.toBeNull()
  expect(screen.getByText(/^Today /)).not.toBeNull()
})

it('keeps the whole Needs a key badge of a model alias on a line of its own', async () => {
  responses['/api/v1/settings/model-aliases'] = {
    items: [
      {
        alias: 'speak',
        candidates: ['openai/gpt-4o-mini-tts', 'elevenlabs/eleven-v3'],
        reachable: false,
        settings: [],
        updated_at: 1,
      },
    ],
  }
  mount('/settings/models')
  const badge = await screen.findByText(/^Needs a key for/)
  expect(badge.parentElement?.textContent).toBe(badge.textContent)
})

it('shows the reason of a failed Delete in the Vault', async () => {
  responses['/api/v1/settings/credentials'] = {
    items: [
      {
        id: 'cred-1',
        domain: 'example.com',
        username: 'alice@example.com',
        login_url: 'https://example.com/login',
        provenance: 'user_supplied',
        has_totp: false,
        owner_agent_id: null,
        created_at: 1,
      },
    ],
  }
  api.DELETE.mockResolvedValue({ error: refused, response: new Response(null, { status: 409 }) })
  mount('/settings/vault')
  fireEvent.click(await screen.findByText('example.com'))
  fireEvent.click(await screen.findByRole('button', { name: 'Delete' }))
  expect((await screen.findByRole('alert')).textContent).toBe('The daemon says no.')
})

it('shows the reason of a failed timezone Save', async () => {
  const device = Intl.DateTimeFormat().resolvedOptions().timeZone
  responses['/api/v1/workspace'] = {
    ...(shellResponse('/api/v1/workspace').data as object),
    timezone: device === 'Pacific/Auckland' ? 'UTC' : 'Pacific/Auckland',
  }
  api.PUT.mockResolvedValue({ error: refused, response: new Response(null, { status: 409 }) })
  mount('/settings/timezone')
  fireEvent.click(await screen.findByRole('button', { name: 'Use it' }))
  fireEvent.click(screen.getByRole('button', { name: 'Save' }))
  expect((await screen.findByRole('alert')).textContent).toBe('The daemon says no.')
})

it('shows the reason of a failed add and a failed Remove in Trusted contacts', async () => {
  responses['/api/v1/settings/trust-list'] = {
    items: [
      { id: 'trust-1', agent_id: null, subject: 'number', value: '+14155550123', tier: 'trusted', label: 'Ana' },
    ],
    own_addresses: [],
    keypad_code: { configured: false },
  }
  api.DELETE.mockResolvedValue({ error: refused, response: new Response(null, { status: 409 }) })
  api.POST.mockResolvedValue({ error: refused, response: new Response(null, { status: 409 }) })
  mount('/settings/trusted-contacts')
  fireEvent.click(await screen.findByText('Ana'))
  fireEvent.click(screen.getByRole('button', { name: 'Remove +14155550123' }))
  expect((await screen.findByRole('alert')).textContent).toBe('The daemon says no.')
  fireEvent.click(screen.getByRole('button', { name: 'Add a trusted contact' }))
  const sheet = await screen.findByRole('dialog', { name: 'Add a trusted contact' })
  fireEvent.change(within(sheet).getByLabelText(/Phone number, email address or domain/), {
    target: { value: '+14155550199' },
  })
  fireEvent.click(within(sheet).getByRole('button', { name: 'Add' }))
  expect((await within(sheet).findByRole('alert')).textContent).toBe('The daemon says no.')
})

it('names the provider of a connection by its catalog label', async () => {
  responses['/api/v1/settings/connections'] = {
    items: [
      {
        id: 'conn-1',
        provider: 'caldav',
        alias: 'calendar',
        display_name: 'Work calendar',
        account: 'ana@example.com',
        status: 'connected',
        capabilities: [],
        authorized_capabilities: [],
      },
    ],
  }
  responses['/api/v1/settings/connections/providers'] = {
    items: [{ id: 'caldav', kind: 'fields', label: 'CalDAV calendar', fields: [], set_up: true }],
  }
  mount('/settings/connections')
  expect(await screen.findByText('CalDAV calendar account')).not.toBeNull()
  expect(screen.queryByText('caldav account')).toBeNull()
})
