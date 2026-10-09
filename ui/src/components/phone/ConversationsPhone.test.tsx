// The Conversations tab of the phone: direct conversations under
// "Sprites" with their last message and the Activity of the Agent,
// groups under "Groups", a search over both, and the New group sheet.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  Outlet,
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
} from '@tanstack/react-router'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { usePresence } from '../../state/presence'
import { shellResponse } from '../../test/appStub'
import { ConversationsPhone } from './ConversationsPhone'

const AT = Date.parse('2026-10-08T09:12:00Z')

const channels = [
  {
    id: 'channel-1', workspace_id: 'workspace-1', kind: 'dm', title: 'Brownie', agent_ids: ['agent-1'], user_member: true, created_at: 1, updated_at: 1,
    last_message: { author_kind: 'agent', author_agent_id: 'agent-1', created_at: AT, text_content: 'Which flight do you want?' },
  },
  {
    id: 'channel-2', workspace_id: 'workspace-1', kind: 'dm', title: 'Pixie', agent_ids: ['agent-2'], user_member: true, created_at: 2, updated_at: 2,
    last_message: { author_kind: 'agent', author_agent_id: 'agent-2', created_at: AT, text_content: 'The invoice is out.' },
  },
  {
    id: 'channel-3', workspace_id: 'workspace-1', kind: 'group', title: 'Austin trip', agent_ids: ['agent-1', 'agent-2'], user_member: true, created_at: 3, updated_at: 3,
    last_message: { author_kind: 'agent', author_agent_id: 'agent-2', created_at: AT, text_content: 'I put the hotel on your calendar.' },
  },
  {
    id: 'channel-4', workspace_id: 'workspace-1', kind: 'group', title: 'Pixie and Brownie', agent_ids: ['agent-1', 'agent-2'], user_member: false, created_at: 4, updated_at: 4,
    last_message: { author_kind: 'agent', author_agent_id: 'agent-1', created_at: AT, text_content: 'I booked the car.' },
  },
]

const agents = [
  { id: 'agent-1', name: 'Brownie', job: 'Travel', status: 'active' },
  { id: 'agent-2', name: 'Pixie', job: 'Finance', status: 'active' },
]

function mount() {
  const api = {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/channels') return { data: { items: channels } }
      if (path === '/api/v1/agents') return { data: { items: agents } }
      if (path === '/api/v1/workspace') return { data: { id: 'workspace-1', name: 'Workspace', timezone: 'UTC', chief_of_staff_agent_id: null } }
      return shellResponse(path)
    }),
    POST: vi.fn(),
  } as unknown as ApiClient
  const history = createMemoryHistory({ initialEntries: ['/conversations'] })
  const rootRoute = createRootRoute({ component: () => <Outlet /> })
  const router = createRouter({
    routeTree: rootRoute.addChildren([
      createRoute({
        getParentRoute: () => rootRoute,
        path: '/conversations',
        component: () => <ConversationsPhone api={api} />,
        validateSearch: (search: Record<string, unknown>): { new?: 'group' } => (search.new === 'group' ? { new: 'group' } : {}),
      }),
      createRoute({ getParentRoute: () => rootRoute, path: '/c/$channelId', component: () => <p>the conversation</p> }),
    ]),
    history,
  })
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <RouterProvider router={router as never} />
    </QueryClientProvider>,
  )
  return history
}

/** The row of the conversation named `title`. */
async function row(title: string): Promise<HTMLElement> {
  return (await screen.findByText(title, { selector: 'strong' })).closest('.ui-row') as HTMLElement
}

function section(name: string): HTMLElement {
  return screen.getByText(name, { selector: '.ui-section-label' }).closest('section') as HTMLElement
}

beforeEach(() => {
  usePresence.setState({ runs: {}, onCall: {}, unread: {}, seeded: true })
})

describe('ConversationsPhone', () => {
  it('lists the direct conversations under Sprites with their last message', async () => {
    mount()

    const brownie = await row('Brownie')
    expect(section('Sprites').contains(brownie)).toBe(true)
    expect(within(brownie).getByText('Which flight do you want?')).toBeTruthy()
    expect(section('Groups').contains(await row('Austin trip'))).toBe(true)
  })

  it("shows the Activity of each Agent on its row", async () => {
    mount()
    await row('Brownie')

    act(() =>
      usePresence.setState({
        runs: {
          'run-1': { runId: 'run-1', agentId: 'agent-1', channelId: 'channel-1', originChannelId: null, state: 'waiting_for_user', caption: 'Waiting' },
          'run-2': { runId: 'run-2', agentId: 'agent-2', channelId: 'channel-2', originChannelId: null, state: 'running', caption: 'Writing the October invoice' },
        },
      }),
    )

    expect(within(await row('Brownie')).getByText('Needs you')).toBeTruthy()
    const pixie = await row('Pixie')
    expect(within(pixie).getByText('Working')).toBeTruthy()
    expect(within(pixie).getByText('Writing the October invoice')).toBeTruthy()
  })

  it('says an Agent-to-Agent conversation is read only, even with a last message', async () => {
    mount()

    const agentsOnly = await row('Pixie and Brownie')
    expect(within(agentsOnly).getByText('You can read this conversation. You cannot post in it.')).toBeTruthy()
    expect(within(agentsOnly).queryByText(/I booked the car/)).toBeNull()
  })

  it('filters the rows by the search', async () => {
    mount()
    await row('Brownie')

    fireEvent.change(screen.getByRole('textbox', { name: 'Search conversations' }), { target: { value: 'aus' } })

    expect(await row('Austin trip')).toBeTruthy()
    expect(screen.queryByText('Brownie', { selector: 'strong' })).toBeNull()
    expect(screen.queryByText('Sprites', { selector: '.ui-section-label' })).toBeNull()
  })

  it('opens the New group sheet from the compose button', async () => {
    const history = mount()

    fireEvent.click(await screen.findByRole('button', { name: 'New group' }))

    expect(await screen.findByRole('dialog', { name: 'New group' })).toBeTruthy()
    await waitFor(() => expect(history.location.search).toContain('new=group'))
  })

  it('opens a conversation from its row', async () => {
    const history = mount()

    fireEvent.click(await row('Pixie'))

    await waitFor(() => expect(history.location.pathname).toBe('/c/channel-2'))
  })
})
