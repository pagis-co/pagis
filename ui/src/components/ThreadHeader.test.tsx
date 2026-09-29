// The Thread header: the Agent's name and a live status line,
// which open the Agent, and the acts on the conversation — Call, Speak
// replies, Search this conversation and the Desk panel.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { shellResponse } from '../test/appStub'
import { renderInRouter } from '../test/router'
import { usePresence } from '../state/presence'
import { useCallInspector, useSpeaking, useThreadSearch } from '../state/stores'
import { ThreadHeader, type ThreadPanel } from './ThreadHeader'

function mount(
  panel?: ThreadPanel,
  panelIsRouteOwned = false,
  channelId = 'channel-1',
) {
  const onTogglePanel = vi.fn()
  const api = { GET: vi.fn(async (path: string) => shellResponse(path)) }
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  const history = renderInRouter(
    <QueryClientProvider client={queryClient}>
      <ThreadHeader
        api={api as unknown as ApiClient}
        channelId={channelId}
        panel={panel}
        panelIsRouteOwned={panelIsRouteOwned}
        onTogglePanel={onTogglePanel}
      />
    </QueryClientProvider>,
  )
  return { onTogglePanel, history }
}

describe('ThreadHeader', () => {
  beforeEach(() => {
    usePresence.setState({ runs: {}, onCall: {}, unread: {}, selectedChannelId: null, seeded: true })
    useSpeaking.setState({ byScope: {}, spoken: {} })
    useThreadSearch.setState({ byChannel: {} })
    useCallInspector.getState().close()
  })

  it('names the Agent and says what it does now', async () => {
    usePresence.setState({
      runs: {
        'run-1': {
          runId: 'run-1',
          agentId: 'agent-1',
          channelId: 'channel-1',
          originChannelId: null,
          state: 'running',
          caption: 'Working on your message',
        },
      },
    })
    mount()

    expect(await screen.findByRole('heading', { name: 'Sage' })).toBeTruthy()
    expect(screen.getByText('Working on your message')).toBeTruthy()
  })

  // With no Run, the header says the Agent's Activity. A Computer word
  // (asleep, awake) says nothing about the Agent's work.
  it('says Idle for an Agent with no work', async () => {
    mount()

    expect(await screen.findByText('Idle')).toBeTruthy()
    expect(screen.queryByText('Asleep')).toBeNull()
  })

  // The Agent's details, memory and settings are one click from the
  // conversation.
  it('opens the Agent from the name and the status line', async () => {
    const { history } = mount()
    const link = await screen.findByRole('link', { name: 'Open Sage' })
    expect(link.getAttribute('href')).toBe('/sprites/agent-1')
    expect(link.querySelector('button')).toBeNull()
    expect(link.querySelector('.sprite-avatar')?.getAttribute('aria-label')).toBe('Sage, Pixie avatar')

    fireEvent.click(link.querySelector('.ui-avatar')!)

    expect(await screen.findByTestId('agent-view')).toBeTruthy()
    expect(history.location.pathname).toBe('/sprites/agent-1')
  })

  // A conversation of more than one Agent names no single Agent, so
  // the header holds no link.
  it('links no Agent from a group conversation', async () => {
    mount(undefined, false, 'channel-2')

    expect(await screen.findByRole('heading', { name: 'Launch planning' })).toBeTruthy()
    expect(screen.queryByRole('link')).toBeNull()
  })

  // A conversation that did not load has no title and no kind yet:
  // the header does not call it an untitled group conversation.
  it('names no title and no kind while the conversations load', async () => {
    const api = { GET: vi.fn(() => new Promise(() => {})) }
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    })
    renderInRouter(
      <QueryClientProvider client={queryClient}>
        <ThreadHeader
          api={api as unknown as ApiClient}
          channelId="channel-1"
          onTogglePanel={vi.fn()}
        />
      </QueryClientProvider>,
    )

    expect(await screen.findByRole('button', { name: 'Call' })).toBeTruthy()
    expect(screen.queryByRole('heading')).toBeNull()
    expect(screen.queryByText('Untitled')).toBeNull()
    expect(screen.queryByText('Group conversation')).toBeNull()
  })

  it('names no title and no kind for a conversation that is not in the list', async () => {
    mount(undefined, false, 'channel-missing')

    expect(await screen.findByRole('button', { name: 'Call' })).toBeTruthy()
    expect(screen.queryByRole('heading')).toBeNull()
    expect(screen.queryByText('Untitled')).toBeNull()
    expect(screen.queryByText('Group conversation')).toBeNull()
  })

  it('joins the call the Agent is on, and offers none without one', async () => {
    mount()
    const call = await screen.findByRole('button', { name: 'Call Sage' })
    expect((call as HTMLButtonElement).disabled).toBe(true)

    act(() => usePresence.setState({ onCall: { 'agent-1': 'call-7' } }))
    fireEvent.click(screen.getByRole('button', { name: 'Call Sage' }))
    expect(useCallInspector.getState().callId).toBe('call-7')
  })

  it('speaks the replies of this conversation', async () => {
    mount()
    const speak = await screen.findByRole('button', { name: 'Speak replies' })
    expect(speak.getAttribute('aria-pressed')).toBe('false')

    fireEvent.click(speak)

    expect(useSpeaking.getState().byScope['channel-1']).toBe(true)
    expect(speak.getAttribute('aria-pressed')).toBe('true')
  })

  it('searches this conversation, and clears the search when it closes', async () => {
    mount()
    fireEvent.click(await screen.findByRole('button', { name: 'Search this conversation' }))

    fireEvent.change(screen.getByRole('textbox', { name: 'Search this conversation' }), {
      target: { value: 'paris' },
    })
    expect(useThreadSearch.getState().byChannel['channel-1']).toBe('paris')

    fireEvent.click(screen.getByRole('button', { name: 'Search this conversation' }))
    expect(useThreadSearch.getState().byChannel['channel-1']).toBe('')
    expect(screen.queryByRole('textbox', { name: 'Search this conversation' })).toBeNull()
  })

  it('toggles the Desk panel', async () => {
    const { onTogglePanel } = mount('desk')
    const desk = await screen.findByRole('button', { name: 'Desk panel' })
    expect(desk.getAttribute('aria-pressed')).toBe('true')

    fireEvent.click(desk)

    expect(onTogglePanel).toHaveBeenCalledWith('desk')
  })

  // The Chief of Staff's channel owns its Desk Panel by the route
  // (ADR-0022), so there is nothing to press.
  it('offers no Desk toggle where the route owns the panel', async () => {
    mount('desk', true)

    await screen.findByRole('button', { name: 'Search this conversation' })
    expect(screen.queryByRole('button', { name: 'Desk panel' })).toBeNull()
  })
})
