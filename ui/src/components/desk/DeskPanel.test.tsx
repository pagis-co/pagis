// The Desk Panel (ADR-0022): the Chief of Staff's Desk expanded
// with the steps of its live Run, the other Desks compact under it,
// and the footer that counts the office and its disk.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { usePresence } from '../../state/presence'
import { DeskPanel } from './DeskPanel'
import type { DeskSurface } from './desks'

const agents = [
  { id: 'ag-1', name: 'Sage', job: 'general assistant', personality: '', status: 'active' },
  { id: 'ag-2', name: 'Clown', job: 'jester', personality: '', status: 'active' },
]

const steps = {
  run_id: 'run-1',
  state: 'running',
  worked_ms: 30_000,
  steps: [
    {
      index: 1,
      kind: 'desk',
      label: 'Woke the computer',
      live: false,
      started_at: 1,
      duration_ms: 21_000,
      screenshot_id: null,
    },
    {
      index: 2,
      kind: 'calendar',
      label: 'Read your calendar',
      live: true,
      started_at: 2,
      duration_ms: null,
      screenshot_id: null,
    },
  ],
}

function stubApi(
  chief: string | null = 'ag-1',
  computerState = 'off',
  diskBytes: number | null = 3_100_000_000,
) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/agents') return { data: { items: agents } }
      if (path === '/api/v1/workspace') {
        return {
          data: {
            id: 'ws-1',
            name: 'Workspace',
            timezone: 'UTC',
            chief_of_staff_agent_id: chief,
          },
        }
      }
      if (path === '/api/v1/settings/onboarding') {
        return { data: { completed: true, docker: { endpoint: 'unix:///var/run/docker.sock', candidates: [] }, docker_endpoint: null, providers: [] } }
      }
      if (path === '/api/v1/agents/{agent_id}/computer') {
        return { data: { state: computerState, percent: null, holder: 'agent' } }
      }
      if (path === '/api/v1/computers/disk') {
        return { data: { bytes: diskBytes } }
      }
      if (path === '/api/v1/runs/{run_id}/steps') return { data: steps }
      return { data: { items: [] } }
    }),
    POST: vi.fn(async (path: string) => {
      if (path === '/api/v1/agents/{agent_id}/screen/offer') {
        return { data: { sdp: 'v=0 answer' } }
      }
      return { data: { state: 'off', percent: null, holder: 'agent' } }
    }),
    PUT: vi.fn(async () => ({ data: {} })),
  }
}

function mount(
  api: unknown,
  surface: DeskSurface = 'home',
  channelId: string | null = null,
  queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  }),
) {
  const onOpenAgent = vi.fn()
  const onOpenChannel = vi.fn()
  const onHire = vi.fn()
  render(
    <QueryClientProvider client={queryClient}>
      <DeskPanel
        api={api as unknown as ApiClient}
        surface={surface}
        channelId={channelId}
        onOpenAgent={onOpenAgent}
        onOpenChannel={onOpenChannel}
        onHire={onHire}
      />
    </QueryClientProvider>,
  )
  return { onOpenAgent, onOpenChannel, onHire, queryClient }
}

/** An awake Desk draws its live screen, and jsdom carries no WebRTC. */
class FakePeerConnection {
  ontrack: ((event: unknown) => void) | null = null
  addTransceiver = vi.fn()
  createDataChannel = vi.fn(() => ({ readyState: 'connecting', send: vi.fn() }))
  createOffer = vi.fn(async () => ({ type: 'offer', sdp: 'v=0 offer' }))
  setLocalDescription = vi.fn(async () => {})
  setRemoteDescription = vi.fn(async () => {})
  close = vi.fn()
}

beforeEach(() => {
  usePresence.setState({ runs: {}, onCall: {}, unread: {} })
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response('missing', { status: 404 })),
  )
  vi.stubGlobal('RTCPeerConnection', FakePeerConnection)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the Desk Panel', () => {
  it('names the Chief of Staff and gives it the first Desk', async () => {
    mount(stubApi())

    expect(
      await screen.findByRole('heading', { name: 'Sage’s desk' }),
    ).toBeTruthy()
    expect(await screen.findByTestId('computer-tile')).toBeTruthy()
  })

  // The header says the Chief of Staff's Activity, and the Desk says
  // its Computer: one word for each, and never the same word.
  it('names the Activity in the header and the Computer on the Desk', async () => {
    mount(stubApi('ag-1', 'awake'))

    const header = (await screen.findByRole('heading', { name: 'Sage’s desk' }))
      .parentElement!
    expect(within(header).getByText('Idle')).toBeTruthy()
    expect(within(await screen.findByTestId('computer-tile')).getByText('Awake')).toBeTruthy()
  })

  it('lists the other Desks compact under it', async () => {
    mount(stubApi())

    const other = await screen.findByTestId('desk-compact')
    expect(within(other).getByText('Clown')).toBeTruthy()
  })

  it('reads the steps of the live Run, the one that runs first', async () => {
    usePresence.setState({
      runs: {
        'run-1': {
          runId: 'run-1',
          agentId: 'ag-1',
          channelId: 'ch-sage',
          originChannelId: null,
          state: 'running',
          caption: 'Working on your message',
        },
      },
    })
    mount(stubApi())

    const list = await screen.findByTestId('desk-steps')
    const rows = await within(list).findAllByRole('listitem')
    expect(rows[0]?.textContent).toContain('Read your calendar')
    expect(rows[0]?.textContent).toContain('now')
    expect(rows[1]?.textContent).toContain('Woke the computer')
  })

  it('shows no step list when the Chief of Staff has no live Run', async () => {
    mount(stubApi())

    await screen.findByTestId('computer-tile')
    expect(screen.queryByTestId('desk-steps')).toBeNull()
  })

  it('opens the Chief of Staff’s channel from Home', async () => {
    const { onOpenChannel } = mount(stubApi())

    fireEvent.click(await screen.findByRole('button', { name: 'Open thread' }))

    expect(onOpenChannel).toHaveBeenCalledWith('ag-1')
  })

  it('shows the Chief of Staff alone in its own channel', async () => {
    mount(stubApi(), 'channel', 'ch-sage')

    await screen.findByRole('heading', { name: 'Sage’s desk' })
    expect(screen.queryByTestId('desk-compact')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Open thread' })).toBeNull()
  })

  it('adds a delegate’s Desk while the channel waits on it', async () => {
    usePresence.setState({
      runs: {
        'run-2': {
          runId: 'run-2',
          agentId: 'ag-2',
          channelId: 'ch-clown',
          originChannelId: 'ch-sage',
          state: 'running',
          caption: 'Working on your message',
        },
      },
    })
    mount(stubApi(), 'channel', 'ch-sage')

    const other = await screen.findByTestId('desk-compact')
    expect(within(other).getByText('Clown')).toBeTruthy()
  })

  // The full-page view is the user's own doing. The panel leaves the
  // inspector slot whenever a Thread, a Call or a Mail opens, and the
  // computer state it read is held, so the Desk it draws on the way
  // back must still be the one inside the panel.
  it('keeps the awake Desk in the panel when the panel opens again', async () => {
    const api = stubApi('ag-1', 'awake')
    const { queryClient } = mount(api)
    await screen.findByLabelText("Sage's live screen")
    cleanup()

    mount(api, 'home', null, queryClient)

    expect(await screen.findByLabelText("Expand Sage's screen")).toBeTruthy()
    expect(screen.queryByLabelText('Fold the screen view')).toBeNull()
  })

  it('puts an awake desk to sleep from its compact row', async () => {
    const api = stubApi('ag-1', 'awake')
    mount(api)

    fireEvent.click(
      await screen.findByRole('button', { name: 'Put Clown’s computer to sleep' }),
    )

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/computer/sleep',
        { params: { path: { agent_id: 'ag-2' } } },
      ),
    )
  })

  it('offers a wake, not a sleep, on a desk that is off', async () => {
    mount(stubApi())

    expect(
      await screen.findByRole('button', { name: 'Wake Clown’s computer' }),
    ).toBeTruthy()
    expect(
      screen.queryByRole('button', { name: 'Put Clown’s computer to sleep' }),
    ).toBeNull()
  })

  it('counts the desks, the awake ones and the disk', async () => {
    mount(stubApi('ag-1', 'awake'))

    const footer = await screen.findByTestId('desk-footer')
    await waitFor(() => expect(footer.textContent).toContain('2 awake'))
    expect(footer.textContent).toContain('2 desks')
    expect(footer.textContent).toContain('disk 3.1 GB')
  })

  it('leaves the disk out when Docker could not say', async () => {
    mount(stubApi('ag-1', 'off', null))

    const footer = await screen.findByTestId('desk-footer')
    await waitFor(() => expect(footer.textContent).toContain('0 awake'))
    expect(footer.textContent).not.toContain('disk')
  })

  it('says so when the Workspace has no active Agent', async () => {
    mount(stubApi(null), 'channel', 'ch-sage')

    expect(await screen.findByText(/no desk is in use/)).toBeTruthy()
  })
})
