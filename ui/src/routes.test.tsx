// Every view has a URL: a deep URL renders its view and Back
// restores the previous one.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory, type RouterHistory } from '@tanstack/react-router'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from './api/client'
import { App } from './App'
import { useCallInspector, useMailInspector } from './state/stores'
import { shellResponse } from './test/appStub'
import { homeDate } from './components/home/report'

const { api } = vi.hoisted(() => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
}))

vi.mock('./api/client', async () => {
  const actual = await vi.importActual<typeof import('./api/client')>('./api/client')
  return { ...actual, createApiClient: () => api as unknown as ApiClient }
})

vi.mock('./ws/socket', () => ({
  PagisSocket: class {
    start() {}
    stop() {}
    subscribeChannel() {}
    activity() {
      return false
    }
  },
}))

function mount(url: string): RouterHistory {
  const history = createMemoryHistory({ initialEntries: [url] })
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <App history={history} />
    </QueryClientProvider>,
  )
  return history
}

/** The person the daemon answers with, at one role. */
function signedInAs(role: 'administrator' | 'member') {
  api.GET.mockImplementation(async (path: string) =>
    path === '/api/v1/user'
      ? {
          data: { id: 'user-1', name: 'Ada', email: null, role },
          response: { status: 200 },
        }
      : shellResponse(path),
  )
}

beforeEach(() => {
  useCallInspector.setState({ callId: null })
  useMailInspector.setState({ mail: null })
  api.POST.mockReset()
  api.GET.mockReset()
  api.GET.mockImplementation(async (path: string) => shellResponse(path))
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response('missing', { status: 404 })),
  )
})

describe('deep URLs', () => {
  // Home is the landing view: no channel is selected on load, so
  // the reader lands on the queue.
  it('renders Home at the root URL and selects no channel', async () => {
    const history = mount('/')

    // Home heads with the day, not with its own name (ADR-0022).
    expect(await screen.findByTestId('home')).toBeTruthy()
    expect(
      await screen.findByRole('heading', { level: 2, name: homeDate(Date.now()) }),
    ).toBeTruthy()
    expect(history.location.pathname).toBe('/')
    expect(screen.queryByRole('heading', { name: 'Sage', level: 1 })).toBeNull()
  })

  it('lands on the first channel from the conversations URL', async () => {
    const history = mount('/c')

    expect(await screen.findByRole('heading', { name: 'Sage' })).toBeTruthy()
    expect(history.location.pathname).toBe('/c/channel-1')
  })

  it('renders the session page at the address of a Coding Session', async () => {
    mount('/coding/session-1')

    expect(await screen.findByRole('heading', { name: 'Fix the login bug' })).toBeTruthy()
    expect(screen.getByText('Claude Code')).toBeTruthy()
  })

  it('renders the sprites URL', async () => {
    mount('/sprites')

    expect(await screen.findByRole('heading', { name: 'Sprites' })).toBeTruthy()
  })

  it('renders a thread URL with its channel behind it', async () => {
    mount('/c/channel-2/t/message-1')

    expect(
      await screen.findByRole('heading', { name: 'Launch planning' }),
    ).toBeTruthy()
    expect(await screen.findByTestId('thread-pane')).toBeTruthy()
    expect(screen.getByText('Ship the launch plan')).toBeTruthy()
  })

  // The replies thread sits over the Desk panel: its Desk button
  // puts the Desk back in the slot, and its close empties the slot.
  it('goes from the replies thread back to the Desk panel', async () => {
    const history = mount('/c/channel-2/t/message-1')
    await screen.findByTestId('thread-pane')

    fireEvent.click(screen.getByRole('button', { name: 'Desk' }))

    expect(await screen.findByTestId('desk-panel')).toBeTruthy()
    expect(screen.queryByTestId('thread-pane')).toBeNull()
    expect(history.location.pathname).toBe('/c/channel-2')
    expect(history.location.href).toContain('panel=desk')
  })

  it('closes the replies thread', async () => {
    const history = mount('/c/channel-2/t/message-1')
    await screen.findByTestId('thread-pane')

    fireEvent.click(screen.getByRole('button', { name: 'Close thread' }))

    await waitFor(() => expect(screen.queryByTestId('thread-pane')).toBeNull())
    expect(history.location.pathname).toBe('/c/channel-2')
  })

  // ADR-0022 keeps the slot precedence: a Call wins over the thread.
  it('keeps a Call over the replies thread', async () => {
    useCallInspector.getState().open('call_1')
    mount('/c/channel-2/t/message-1')

    expect(await screen.findByTestId('call-inspector')).toBeTruthy()
    expect(screen.queryByTestId('thread-pane')).toBeNull()
  })

  // The daemon answers 404 for a channel that is not the Person's, so
  // nothing leaks; the view says the conversation does not exist and
  // does not call it a failed load.
  it('says a conversation that is not the person\'s does not exist', async () => {
    api.GET.mockImplementation(async (path: string) =>
      path === '/api/v1/channels/{channel_id}/messages'
        ? {
            error: { error: { code: 'not_found', message: 'channel not found' } },
            response: { status: 404 },
          }
        : shellResponse(path),
    )
    mount('/c/channel-of-someone-else')

    expect(await screen.findByText('This conversation does not exist')).toBeTruthy()
    expect(screen.queryByText('Could not load this conversation')).toBeNull()
    expect(screen.queryByText('Group conversation')).toBeNull()
    expect(screen.queryByRole('textbox', { name: /message/i })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Go to Home' }))
    expect(await screen.findByTestId('home')).toBeTruthy()
  })

  it('renders the runs URL', async () => {
    mount('/runs')

    expect(await screen.findByRole('heading', { name: 'Runs' })).toBeTruthy()
  })

  it('renders a settings section URL', async () => {
    mount('/settings/retention')

    expect(await screen.findByRole('heading', { name: 'Retention' })).toBeTruthy()
  })

  // The sidebar's Settings opens the first section; the bare address
  // opens the same view.
  it('opens the first settings section at the settings URL', async () => {
    const history = mount('/settings')

    expect(await screen.findByRole('heading', { name: 'Connections' })).toBeTruthy()
    expect(history.location.pathname).toBe('/settings/connections')
  })

  it('opens a connection as its own page and returns to the list', async () => {
    const connection = {
      id: 'conn-1', provider: 'google', capabilities: ['mail'], alias: 'personal',
      display_name: 'Google', status: 'connected', auth_mode: 'byo',
      account: 'alice@example.com', authorized_capabilities: ['gmail_read'], created_at: 1,
    }
    api.GET.mockImplementation(async (path: string) =>
      path === '/api/v1/settings/connections'
        ? { data: { items: [connection] } }
        : shellResponse(path),
    )
    const history = mount('/settings/connections')

    fireEvent.click(await screen.findByRole('button', { name: 'Open Google' }))
    expect(
      await screen.findByRole('heading', { name: 'Google · alice@example.com' }),
    ).toBeTruthy()
    expect(history.location.pathname).toBe('/settings/connections/conn-1')

    fireEvent.click(screen.getByRole('button', { name: 'Back to Connections' }))
    await waitFor(() => expect(history.location.pathname).toBe('/settings/connections'))
  })

  it('renders an agent URL as the profile', async () => {
    mount('/sprites/agent-1')

    expect(await screen.findByRole('heading', { name: 'Sage' })).toBeTruthy()
    expect(screen.getByTestId('agent-profile')).toBeTruthy()
    expect(screen.getByRole('tab', { name: 'Desk' })).toBeTruthy()
  })

  it('opens the creating flow from /sprites?new=1', async () => {
    mount('/sprites?new=1')

    expect(await screen.findByLabelText('Sprite name')).toBeTruthy()
  })

  it('lands on the new agent DM when the four create steps finish', async () => {
    api.GET.mockImplementation(async (path: string) => {
      if (path === '/api/v1/channels') {
        return {
          data: {
            items: [
              {
                id: 'channel-9',
                workspace_id: 'workspace-1',
                kind: 'dm',
                title: 'Rex',
                agent_ids: ['agent-9'],
                user_member: true,
                created_at: 3,
                updated_at: 3,
              },
            ],
          },
        }
      }
      return shellResponse(path)
    })
    api.POST.mockResolvedValue({ data: { id: 'agent-9', name: 'Rex' } })
    const history = mount('/sprites?new=1')

    fireEvent.change(await screen.findByLabelText('Sprite name'), {
      target: { value: 'Rex' },
    })
    fireEvent.click(screen.getByText('Next'))
    fireEvent.change(await screen.findByLabelText('Sprite job'), {
      target: { value: 'researcher' },
    })
    fireEvent.click(screen.getByText('Next'))
    fireEvent.click(await screen.findByText('Next'))
    fireEvent.click(await screen.findByText('Create'))

    await waitFor(() => expect(history.location.pathname).toBe('/c/channel-9'))
  })

  // The installation's settings answer on the administration port, so
  // an administrator's one section is the link to it.
  it('opens the Administration section for an administrator', async () => {
    signedInAs('administrator')

    const history = mount('/settings/administration')

    expect(await screen.findByRole('button', { name: 'Administration' })).toBeTruthy()
    expect(history.location.pathname).toBe('/settings/administration')
  })

  it('sends a member away from the Administration section and hides it', async () => {
    signedInAs('member')

    const history = mount('/settings/administration')

    await waitFor(() =>
      expect(history.location.pathname).toBe('/settings/connections'),
    )
    expect(screen.queryByRole('button', { name: 'Administration' })).toBeNull()
  })

  it('sends the address of an administration section to the sprites board', async () => {
    signedInAs('administrator')

    const history = mount('/settings/system')

    await waitFor(() => expect(history.location.pathname).toBe('/sprites'))
  })

  it('sends an address that names no settings section to the sprites board', async () => {
    const history = mount('/settings/unknown')

    expect(await screen.findByTestId('sprite-roster')).toBeTruthy()
    expect(history.location.pathname).toBe('/sprites')
  })

  it('renders the automations and software URLs', async () => {
    mount('/automations')
    expect(await screen.findByRole('heading', { name: 'Automations' })).toBeTruthy()
  })

  // Memory opens on the Chief of Staff; the URL holds the
  // scope, the page and the view.
  it('renders the memory URL on the Chief of Staff', async () => {
    mount('/memory')

    const scopes = await screen.findByRole('group', { name: 'Scope' })
    const sage = await within(scopes).findByRole('button', { name: 'Sage' })
    expect(sage.getAttribute('aria-pressed')).toBe('true')
    expect(api.GET).toHaveBeenCalledWith('/api/v1/memory/pages', {
      params: { query: { scope: 'agent:agent-1' } },
    })
  })

  it('puts the memory scope and view in the address bar', async () => {
    const history = mount('/memory?scope=agent%3Aagent-1&view=procedures')

    const views = await screen.findByRole('group', { name: 'Views' })
    expect(
      within(views).getByRole('button', { name: 'Procedures' }).getAttribute('aria-pressed'),
    ).toBe('true')
    fireEvent.click(await screen.findByRole('button', { name: 'Shared' }))

    await waitFor(() =>
      expect(history.location.search).toBe('?scope=shared&view=procedures'),
    )
  })

  // The durable inspector tenants live in the URL (ADR-0022); Call and
  // Mail stay transient in their stores.
  it('opens the Desk Panel from the panel search parameter', async () => {
    mount('/c/channel-2?panel=desk')

    expect(await screen.findByTestId('desk-panel')).toBeTruthy()
  })
})

describe('history', () => {
  it('restores the previous view when the user goes back', async () => {
    const history = mount('/c/channel-1')

    await screen.findByRole('heading', { name: 'Sage' })

    fireEvent.click(screen.getByRole('button', { name: /Launch planning/ }))
    expect(
      await screen.findByRole('heading', { name: 'Launch planning' }),
    ).toBeTruthy()

    const navigation = screen.getByRole('navigation', { name: 'Places' })
    fireEvent.click(within(navigation).getByRole('button', { name: 'Automations' }))
    expect(await screen.findByRole('heading', { name: 'Automations' })).toBeTruthy()
    expect(history.location.pathname).toBe('/automations')

    act(() => history.back())
    expect(
      await screen.findByRole('heading', { name: 'Launch planning' }),
    ).toBeTruthy()
    expect(history.location.pathname).toBe('/c/channel-2')

    act(() => history.back())
    expect(await screen.findByRole('heading', { name: 'Sage' })).toBeTruthy()
    expect(history.location.pathname).toBe('/c/channel-1')
  })

  it('puts the header toggles in the address bar', async () => {
    const history = mount('/c/channel-2')

    await screen.findByRole('heading', { name: 'Launch planning' })
    fireEvent.click(screen.getByRole('button', { name: 'Desk panel' }))

    await waitFor(() => expect(history.location.href).toContain('panel=desk'))
  })

  it('opens no inspector for a panel value it does not know', async () => {
    mount('/c/channel-2?panel=unknown')

    await screen.findByRole('heading', { name: 'Launch planning' })
    expect(document.querySelector('.workspace-inspector')).toBeNull()
  })
})


// Between the phone and the wide screen, the inspector is a column the
// person opens and closes, so it never covers the composer (ADR-0022).
describe('a window of 760px to 1100px', () => {
  beforeEach(() => {
    vi.stubGlobal('matchMedia', (query: string) => ({
      // 1000px wide: under the compact breakpoint, over the phone one.
      matches: query === '(max-width: 1100px)',
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      dispatchEvent: () => false,
    }))
  })

  it('opens no Desk Panel by itself on Home, and the person opens and closes it', async () => {
    const history = mount('/')

    await screen.findByTestId('home')
    expect(screen.queryByTestId('desk-panel')).toBeNull()

    fireEvent.click(screen.getByRole('button', { name: 'Desk panel' }))
    expect(await screen.findByTestId('desk-panel')).toBeTruthy()
    expect(history.location.href).toContain('panel=desk')

    fireEvent.click(screen.getByRole('button', { name: 'Desk panel' }))
    await waitFor(() => expect(screen.queryByTestId('desk-panel')).toBeNull())
    expect(history.location.href).not.toContain('panel=desk')
  })

  it("opens no Desk Panel by itself in the Chief of Staff's channel, and offers the toggle", async () => {
    const history = mount('/c/channel-1')

    await screen.findByRole('heading', { name: 'Sage' })
    expect(screen.queryByTestId('desk-panel')).toBeNull()

    fireEvent.click(screen.getByRole('button', { name: 'Desk panel' }))
    expect(await screen.findByTestId('desk-panel')).toBeTruthy()
    expect(history.location.href).toContain('panel=desk')
  })
})

describe('a wide window', () => {
  beforeEach(() => {
    vi.stubGlobal('matchMedia', (query: string) => ({
      matches: false,
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      dispatchEvent: () => false,
    }))
  })

  // Home is the office, so its Desk Panel is open before the user asks
  // (ADR-0022).
  it('opens the Desk Panel by itself on Home, with no toggle', async () => {
    mount('/')

    expect(await screen.findByTestId('desk-panel')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Desk panel' })).toBeNull()
  })
})
