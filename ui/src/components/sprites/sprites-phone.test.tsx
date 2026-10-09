// The Sprites tab at phone width: the roster, the New sprite sheet,
// the Access switch list with its Coding sessions section, and the Memory and Work screens of a sprite.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory } from '@tanstack/react-router'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
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

const google = {
  id: 'c-1',
  provider: 'google',
  alias: 'google-1',
  display_name: 'Google',
  account: 'ana@gmail.com',
  status: 'connected',
  capabilities: [],
  absent_capabilities: [],
  authorized_capabilities: ['gmail_read', 'calendar_read', 'gmail_send'],
  installation: false,
  created_at: 1,
}

const grant = (capabilities: string[]) => ({
  id: 'grant-1',
  agent_id: 'agent-1',
  agent_name: 'Sage',
  resource_kind: 'connection',
  resource_id: 'c-1',
  capabilities,
  allow: [],
  sessions: [],
  revision: 1,
  created_at: 1,
})

const run = (id: string, title: string, created_at: number) => ({
  id,
  agent_id: 'agent-1',
  channel_id: 'channel-1',
  title,
  state: 'completed',
  trigger_kind: 'message',
  duration_ms: 51_000,
  created_at,
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
  return history
}

it('lists each sprite with its job and opens its profile', async () => {
  const history = mount('/sprites')
  const row = await screen.findByTestId('sprite-row')
  expect(row.textContent).toContain('Sage')
  expect(row.textContent).toContain('General assistant')
  expect(row.textContent).toContain('Main sprite')
  fireEvent.click(row)
  await waitFor(() => expect(history.location.pathname).toBe('/sprites/agent-1'))
})

it('makes a sprite from the New sprite sheet and opens its conversation', async () => {
  api.POST.mockImplementation(async (path: string) =>
    path === '/api/v1/agents' ? { data: { id: 'agent-1', name: 'Pebble' } } : { data: {} },
  )
  const history = mount('/sprites')
  fireEvent.click(await screen.findByRole('button', { name: 'New sprite' }))
  const sheet = await screen.findByRole('dialog', { name: 'New sprite' })
  const create = within(sheet).getByRole('button', { name: 'Create' })
  expect((create as HTMLButtonElement).disabled).toBe(true)
  fireEvent.change(within(sheet).getByLabelText('Name'), { target: { value: 'Pebble' } })
  fireEvent.change(within(sheet).getByLabelText('Job'), { target: { value: 'Bookkeeping' } })
  fireEvent.click(create)
  await waitFor(() =>
    expect(api.POST).toHaveBeenCalledWith(
      '/api/v1/agents',
      expect.objectContaining({ body: expect.objectContaining({ name: 'Pebble', job: 'Bookkeeping' }) }),
    ),
  )
  await waitFor(() => expect(history.location.pathname).toBe('/c/channel-1'))
})

it('goes back from the voice step with Done before Name and Job are written', async () => {
  mount('/sprites?new=1')
  const sheet = await screen.findByRole('dialog', { name: 'New sprite' })
  fireEvent.click(within(sheet).getByRole('button', { name: /Personality and voice/ }))
  const voice = await screen.findByRole('dialog', { name: 'Personality and voice' })
  const done = within(voice).getByRole('button', { name: 'Done' })
  expect((done as HTMLButtonElement).disabled).toBe(false)
  fireEvent.click(done)
  expect(await screen.findByRole('dialog', { name: 'New sprite' })).not.toBeNull()
})

it('says what the mailbox lacks when Create meets an unfinished one', async () => {
  responses['/api/v1/settings/mailbox-offers'] = {
    offers: [
      {
        connection_id: 'mx-1',
        display_name: 'Mail host',
        domain: 'pagis.example',
        suggested_local_part: 'pebble',
        default_outgoing_cap: 50,
        mints_password: false,
        deletes_mailbox: false,
      },
    ],
  }
  mount('/sprites?new=1')
  const sheet = await screen.findByRole('dialog', { name: 'New sprite' })
  fireEvent.change(within(sheet).getByLabelText('Name'), { target: { value: 'Pebble' } })
  fireEvent.change(within(sheet).getByLabelText('Job'), { target: { value: 'Bookkeeping' } })
  fireEvent.click(within(sheet).getByRole('button', { name: /What it may reach/ }))
  const access = await screen.findByRole('dialog', { name: 'What it may reach' })
  fireEvent.click(await within(access).findByRole('switch', { name: /its own mailbox/ }))
  fireEvent.click(within(access).getByRole('button', { name: 'Done' }))
  const main = await screen.findByRole('dialog', { name: 'New sprite' })
  fireEvent.click(within(main).getByRole('button', { name: 'Create' }))
  expect((await within(main).findByRole('alert')).textContent).toBe(
    'Finish the mailbox, or leave it out.',
  )
  expect(api.POST).not.toHaveBeenCalled()
})

it('saves a changed grant with PUT, and revokes it', async () => {
  responses['/api/v1/settings/connections'] = { items: [google] }
  responses['/api/v1/grants'] = { items: [grant(['gmail_read'])] }
  api.PUT.mockResolvedValue({ data: grant(['gmail_read', 'calendar_read']) })
  api.DELETE.mockResolvedValue({ response: new Response(null, { status: 204 }) })
  const history = mount('/sprites/agent-1/access/c-1')
  fireEvent.click(await screen.findByRole('switch', { name: 'Read Calendar' }))
  fireEvent.click(screen.getByRole('button', { name: 'Save access' }))
  await waitFor(() =>
    expect(api.PUT).toHaveBeenCalledWith('/api/v1/grants/{grant_id}/capabilities', {
      params: { path: { grant_id: 'grant-1' } },
      body: { capabilities: ['gmail_read', 'calendar_read'] },
    }),
  )
  expect(api.POST).not.toHaveBeenCalled()
  fireEvent.click(screen.getByRole('button', { name: 'Revoke access' }))
  await waitFor(() =>
    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/grants/{grant_id}', {
      params: { path: { grant_id: 'grant-1' } },
    }),
  )
  await waitFor(() => expect(history.location.pathname).toBe('/sprites/agent-1/access'))
})

it('makes a new grant with POST where the sprite has none', async () => {
  responses['/api/v1/settings/connections'] = { items: [google] }
  api.POST.mockResolvedValue({ data: grant(['gmail_read']) })
  mount('/sprites/agent-1/access/c-1')
  const save = await screen.findByRole('button', { name: 'Save access' })
  expect((save as HTMLButtonElement).disabled).toBe(true)
  expect(screen.queryByRole('button', { name: 'Revoke access' })).toBeNull()
  fireEvent.click(screen.getByRole('switch', { name: 'Read Gmail' }))
  fireEvent.click(save)
  await waitFor(() =>
    expect(api.POST).toHaveBeenCalledWith('/api/v1/grants', {
      body: { agent_id: 'agent-1', connection_id: 'c-1', capabilities: ['gmail_read'] },
    }),
  )
})

it('shows the daemon reason when a Save of access fails', async () => {
  responses['/api/v1/settings/connections'] = { items: [google] }
  api.POST.mockResolvedValue({
    error: { error: { code: 'forbidden', message: 'Only the owner grants this.' } },
    response: new Response(null, { status: 403 }),
  })
  mount('/sprites/agent-1/access/c-1')
  fireEvent.click(await screen.findByRole('switch', { name: 'Read Gmail' }))
  fireEvent.click(screen.getByRole('button', { name: 'Save access' }))
  expect((await screen.findByRole('alert')).textContent).toBe('Only the owner grants this.')
})

const air = {
  id: 'h-1',
  name: 'Air',
  platform: 'macos',
  capabilities: ['shell', 'harness:claude-code'],
  present: true,
  last_seen_at: 1,
}

const hostGrant = (overrides: Record<string, unknown> = {}) => ({
  ...grant([]),
  id: 'grant-2',
  resource_kind: 'host',
  resource_id: 'h-1',
  session_approval_mode: 'person',
  unattended_modes: false,
  ...overrides,
})

it('sets the approval mode of coding sessions on a computer from the Access tab', async () => {
  const user = userEvent.setup()
  responses['/api/v1/hosts'] = {
    items: [air, { ...air, id: 'h-2', name: 'Server', capabilities: ['shell'] }],
  }
  responses['/api/v1/grants'] = { items: [hostGrant()] }
  api.PUT.mockResolvedValue({ data: hostGrant({ session_approval_mode: 'agent' }) })
  mount('/sprites/agent-1/access')
  const section = await screen.findByRole('region', { name: 'Coding sessions' })
  const mode = await within(section).findByRole('combobox', {
    name: 'Approvals for coding sessions on Air',
  })
  expect(within(section).queryByRole('combobox', { name: /Server/ })).toBeNull()
  expect(mode.textContent).toContain('Ask me')
  await user.click(mode)
  await user.click(await screen.findByRole('option', { name: 'Let the sprite decide' }))
  await waitFor(() =>
    expect(api.PUT).toHaveBeenCalledWith(
      '/api/v1/agents/{agent_id}/hosts/{host_id}/session-approval-mode',
      { params: { path: { agent_id: 'agent-1', host_id: 'h-1' } }, body: { mode: 'agent' } },
    ),
  )
})

it('allows modes that act without asking on a computer from the Access tab', async () => {
  responses['/api/v1/hosts'] = { items: [air] }
  api.PUT.mockImplementation(async () => {
    responses['/api/v1/grants'] = { items: [hostGrant({ unattended_modes: true })] }
    return { data: hostGrant({ unattended_modes: true }) }
  })
  mount('/sprites/agent-1/access')
  const section = await screen.findByRole('region', { name: 'Coding sessions' })
  const allow = await within(section).findByRole('switch', {
    name: 'Allow modes that act without asking',
  })
  expect(allow.getAttribute('aria-checked')).toBe('false')
  fireEvent.click(allow)
  await waitFor(() =>
    expect(api.PUT).toHaveBeenCalledWith(
      '/api/v1/agents/{agent_id}/hosts/{host_id}/unattended-modes',
      { params: { path: { agent_id: 'agent-1', host_id: 'h-1' } }, body: { allowed: true } },
    ),
  )
  expect(
    await within(section).findByText(
      'A coding harness can then run any command as you on Air, with no question.',
    ),
  ).not.toBeNull()
})

it('says on the Access tab when no computer can start a coding harness', async () => {
  responses['/api/v1/hosts'] = { items: [{ ...air, capabilities: ['shell'] }] }
  mount('/sprites/agent-1/access')
  const section = await screen.findByRole('region', { name: 'Coding sessions' })
  expect(
    await within(section).findByText(/No computer of yours can start a coding harness/),
  ).not.toBeNull()
})

it('groups the Work of a sprite by day with the title of each run', async () => {
  const now = Date.now()
  responses['/api/v1/runs'] = {
    items: [
      run('run-1', 'Filed the September receipts', now),
      run('run-2', 'Paid the Northwind invoice', now - DAY),
    ],
  }
  const history = mount('/sprites/agent-1/work')
  const today = await screen.findByText('Today')
  const yesterday = screen.getByText('Yesterday')
  expect(
    within(today.closest('section')!).getByText('Filed the September receipts'),
  ).not.toBeNull()
  expect(
    within(yesterday.closest('section')!).getByText('Paid the Northwind invoice'),
  ).not.toBeNull()
  fireEvent.click(screen.getByRole('button', { name: /Filed the September receipts/ }))
  await waitFor(() => expect(history.location.pathname).toBe('/runs/run-1'))
})

it('shows what the Memory of a sprite holds and opens all its changes', async () => {
  responses['/api/v1/memory/feed'] = {
    items: [
      {
        id: 'f-1',
        kind: 'committed',
        sha: 'abc',
        agent_id: 'agent-1',
        agent_name: 'Sage',
        message: 'Noted the Northwind billing day',
        files: ['subjects/northwind.md'],
        run_id: null,
        created_at: Date.now(),
      },
    ],
    revision: 'r1',
  }
  const history = mount('/sprites/agent-1/memory')
  expect(await screen.findByText('Noted the Northwind billing day')).not.toBeNull()
  expect(screen.getByText('Holds')).not.toBeNull()
  fireEvent.click(screen.getByRole('button', { name: /See all/ }))
  await waitFor(() => expect(history.location.pathname).toBe('/memory'))
  expect(history.location.search).toContain('view=changes')
})
