// The sidebar: the seven places with the Needs-You count on Home,
// the conversations with the Chief of Staff first, one current mark at
// a time, and the profile row that names the user and opens Settings.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { usePresence } from '../../state/presence'
import { Sidebar } from './Sidebar'

const agents = [
  { id: 'ag-1', name: 'Sage', job: 'general assistant', personality: '', status: 'active' },
  {
    id: 'ag-2',
    name: 'Clown',
    job: 'Chief Entertainment Officer',
    personality: '',
    status: 'active',
  },
]

const channels = [
  {
    id: 'ch-group',
    workspace_id: 'ws-1',
    kind: 'group',
    agent_ids: ['ag-1', 'ag-2'],
    user_member: true,
    title: 'Ops',
    created_at: 1,
    updated_at: 1,
  },
  // The channel the two Agents opened between themselves (ADR-0003).
  {
    id: 'ch-agents',
    workspace_id: 'ws-1',
    kind: 'dm',
    agent_ids: ['ag-1', 'ag-2'],
    user_member: false,
    title: 'Sage ↔ Clown',
    created_at: 1,
    updated_at: 1,
  },
  {
    id: 'ch-clown',
    workspace_id: 'ws-1',
    kind: 'dm',
    agent_ids: ['ag-2'],
    user_member: true,
    title: 'Clown',
    created_at: 2,
    updated_at: 2,
  },
  {
    id: 'ch-sage',
    workspace_id: 'ws-1',
    kind: 'dm',
    agent_ids: ['ag-1'],
    user_member: true,
    title: 'Sage',
    created_at: 3,
    updated_at: 3,
  },
]

// The daemon's Needs-You Queue: two decisions wait.
const needsYou = {
  items: ['req-1', 'req-2'].map((requestId, index) => ({
    kind: 'approval',
    id: `request:${requestId}`,
    agent_id: 'ag-1',
    line: 'Sage needs your approval',
    url: '/',
    at: index,
    request_id: requestId,
    request_kind: 'tool_action',
    title: 'An action',
    body: '',
  })),
  count: 2,
}

function stubApi(userName: string | null, captureDays: number | null = null) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/channels') return { data: { items: channels } }
      if (path === '/api/v1/agents') return { data: { items: agents } }
      if (path === '/api/v1/workspace') {
        return {
          data: {
            id: 'ws-1',
            name: 'Workspace',
            timezone: 'UTC',
            chief_of_staff_agent_id: 'ag-1',
          },
        }
      }
      if (path === '/api/v1/user') {
        return { data: { name: userName, model_request_capture_days: captureDays } }
      }
      if (path === '/api/v1/needs-you') return { data: needsYou }
      return { data: { items: [] } }
    }),
    POST: vi.fn(async () => ({
      data: {
        id: 'ch-new',
        workspace_id: 'ws-1',
        kind: 'group',
        agent_ids: ['ag-1'],
        user_member: true,
        title: 'Launch',
        created_at: 9,
        updated_at: 9,
      },
    })),
  }
}

function mount(props: {
  pathname: string
  selectedId: string | null
  /** The name the wizard recorded, or `null` for a wizard that took none. */
  userName?: string | null
  /** The retention of Model Request Capture, or `null` while it is off. */
  captureDays?: number | null
}) {
  const onSelectPlace = vi.fn()
  const onSelectChannel = vi.fn()
  const onSearch = vi.fn()
  const onSignOut = vi.fn()
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={queryClient}>
      <Sidebar
        api={stubApi(props.userName ?? null, props.captureDays ?? null) as unknown as ApiClient}
        pathname={props.pathname}
        selectedId={props.selectedId}
        onSelectPlace={onSelectPlace}
        onSelectChannel={onSelectChannel}
        onSearch={onSearch}
        onSignOut={onSignOut}
        open={false}
        onClose={() => {}}
      />
    </QueryClientProvider>,
  )
  return { onSelectPlace, onSelectChannel, onSearch }
}

function places() {
  return screen.getByRole('navigation', { name: 'Places' })
}

function conversations() {
  return screen.getByRole('navigation', { name: 'Conversations' })
}

/** The row whose title is exactly `title`. */
async function row(title: string): Promise<HTMLElement> {
  const titles = await within(conversations()).findAllByTestId('conversation-title')
  const match = titles.find((node) => node.textContent === title)
  if (match === undefined) throw new Error(`no conversation row titled ${title}`)
  return match.closest('button') as HTMLElement
}

/** Sage starts a Run in one channel, as the firehose reports it. */
function startRun(channelId: string, agentId = 'ag-1') {
  usePresence.getState().applyFrame('run.created', {
    id: 'ev-1',
    event_type: 'run.created',
    agent_id: agentId,
    channel_id: channelId,
    run_id: 'run-1',
    payload: { trigger_kind: 'message' },
  })
  usePresence.getState().applyFrame('run.state_changed', {
    id: 'ev-2',
    event_type: 'run.state_changed',
    agent_id: agentId,
    channel_id: channelId,
    run_id: 'run-1',
    payload: { to: 'running' },
  })
}

/** The presence each face of a row carries, in drawing order. */
function faces(node: HTMLElement): (string | null)[] {
  return [...node.querySelectorAll('.ui-avatar')].map((face) =>
    face.getAttribute('data-presence'),
  )
}

describe('Sidebar', () => {
  beforeEach(() => {
    usePresence.setState({
      runs: {},
      onCall: {},
      unread: {},
      selectedChannelId: null,
      seeded: false,
    })
  })

  it('tells the Person when Pagis keeps a copy of their model requests', async () => {
    mount({ pathname: '/', selectedId: null, captureDays: 7 })

    expect(
      await screen.findByText('Pagis keeps a copy of your model requests for 7 days.'),
    ).toBeTruthy()
  })

  it('says nothing about model requests while the capture is off', async () => {
    mount({ pathname: '/', selectedId: null })

    await screen.findByText('Sage')
    expect(screen.queryByText(/keeps a copy of your model requests/)).toBeNull()
  })

  it('lists the seven places, with the Needs-You count on Home', async () => {
    mount({ pathname: '/', selectedId: null })

    const names = within(places())
      .getAllByRole('button')
      .map((button) => button.textContent)
    expect(names).toEqual([
      'Home',
      'Sprites',
      'Memory',
      'Automations',
      'Coding',
      'Software',
      'Settings',
    ])

    const count = await within(places()).findByLabelText('2 need you')
    expect(count.textContent).toBe('2')
  })

  it('navigates from a place', async () => {
    const { onSelectPlace } = mount({ pathname: '/', selectedId: null })
    fireEvent.click(within(places()).getByRole('button', { name: 'Memory' }))
    expect(onSelectPlace).toHaveBeenCalledWith('memory')
  })

  it('marks the place of the path and no conversation', async () => {
    mount({ pathname: '/memory', selectedId: null })
    await screen.findByText('Clown')

    expect(
      within(places()).getByRole('button', { name: 'Memory' }).getAttribute('aria-current'),
    ).toBe('page')
    expect(document.querySelectorAll('[aria-current="page"]').length).toBe(1)
  })

  it('marks the conversation and no place in a thread', async () => {
    mount({ pathname: '/c/ch-clown', selectedId: 'ch-clown' })
    const clown = await row('Clown')

    expect(clown.getAttribute('aria-current')).toBe('page')
    expect(document.querySelectorAll('[aria-current="page"]').length).toBe(1)
  })

  it('draws the Chief of Staff first with a presence ring and a status line', async () => {
    startRun('ch-sage')
    mount({ pathname: '/', selectedId: null })

    const rows = await within(conversations()).findAllByTestId('conversation-title')
    expect(rows.map((row) => row.textContent)).toEqual([
      'Sage',
      'Clown',
      'Ops',
      'Sage ↔ Clown',
    ])

    const chief = await row('Sage')
    expect(chief.querySelector('.ui-avatar')?.getAttribute('data-presence')).toBe('working')
    expect(within(chief).getByTestId('conversation-status').textContent).toBe(
      'Working on your message',
    )
    const clown = await row('Clown')
    expect(within(clown).getByTestId('conversation-status').textContent).toBe(
      'Chief Entertainment Officer · Idle',
    )
  })

  // The Chief of Staff is a designation, not a job: the row names the
  // job the user wrote, as every other row does.
  it('names the job of the Chief of Staff and its Activity', async () => {
    mount({ pathname: '/', selectedId: null })

    const chief = await row('Sage')
    expect(within(chief).getByTestId('conversation-status').textContent).toBe(
      'general assistant · Idle',
    )
  })

  // An Agent at work in its own channel with the user lights up that
  // row alone. A shared channel follows the work done in it (ADR-0022).
  it('rings the Agent’s own channel, not every channel it is in', async () => {
    startRun('ch-sage')
    mount({ pathname: '/', selectedId: null })

    expect(faces(await row('Sage'))).toEqual(['working'])
    expect(faces(await row('Sage ↔ Clown'))).toEqual(['idle', 'idle'])
    expect(faces(await row('Ops'))).toEqual(['idle', 'idle'])
  })

  it('rings a shared channel while the Agent writes in it', async () => {
    startRun('ch-agents')
    mount({ pathname: '/', selectedId: null })

    const shared = await row('Sage ↔ Clown')
    expect(faces(shared)).toEqual(['working', 'idle'])
    expect(within(shared).getByTestId('conversation-status').textContent).toBe(
      'Working on your message',
    )
    // The user's own channel with Sage says Sage works, because the
    // user follows Sage there.
    expect(faces(await row('Sage'))).toEqual(['working'])
  })

  it('opens a conversation from its row', async () => {
    const { onSelectChannel } = mount({ pathname: '/', selectedId: null })
    fireEvent.click(await row('Clown'))
    expect(onSelectChannel).toHaveBeenCalledWith('ch-clown')
  })

  it('starts a group from the + on the heading', async () => {
    const { onSelectChannel } = mount({ pathname: '/', selectedId: null })
    await screen.findByText('Clown')

    fireEvent.click(screen.getByRole('button', { name: 'New group' }))
    fireEvent.change(screen.getByLabelText('Group name'), { target: { value: 'Launch' } })
    fireEvent.click(screen.getByRole('button', { name: 'Create' }))

    await waitFor(() => expect(onSelectChannel).toHaveBeenCalledWith('ch-new'))
  })

  it('heads the column with the mark and the name', () => {
    mount({ pathname: '/', selectedId: null })
    const brand = screen.getByText('Pagis')
    expect(brand.className).toContain('sidebar-brand')
    expect(brand.querySelector('.ui-logo-mark')).not.toBeNull()
  })

  it('opens search from the header icon', () => {
    const { onSearch } = mount({ pathname: '/', selectedId: null })
    fireEvent.click(screen.getByRole('button', { name: 'Search' }))
    expect(onSearch).toHaveBeenCalled()
  })

  it('names the user in the profile row', async () => {
    mount({ pathname: '/', selectedId: null, userName: 'Ada' })
    expect(await screen.findByRole('button', { name: 'Ada' })).toBeTruthy()
  })

  it('calls the user You until the wizard records a name', async () => {
    mount({ pathname: '/', selectedId: null })
    expect(await screen.findByRole('button', { name: 'You' })).toBeTruthy()
  })

  it('opens settings from the profile row', async () => {
    const { onSelectPlace } = mount({ pathname: '/', selectedId: null, userName: 'Ada' })
    fireEvent.click(await screen.findByRole('button', { name: 'Ada' }))
    expect(onSelectPlace).toHaveBeenCalledWith('settings')
  })
})
