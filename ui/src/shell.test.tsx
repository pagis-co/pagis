// One sidebar, one behaviour, and the mobile shell.
// Every place navigates and says it is current; a move to a place puts
// the inspector away; on a phone the drawer, the inspector and the
// thread each open and close with their own control.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory, type RouterHistory } from '@tanstack/react-router'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from './api/client'
import { App } from './App'
import { useCallInspector, useMailInspector, useMobileNav } from './state/stores'
import { shellResponse } from './test/appStub'

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

/** The one breakpoint the shell branches on. */
function setViewport(width: number) {
  vi.stubGlobal(
    'matchMedia',
    (query: string) => ({
      matches: width <= 760,
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      dispatchEvent: () => false,
    }),
  )
}

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

beforeEach(() => {
  setViewport(1440)
  useCallInspector.setState({ callId: null })
  useMailInspector.setState({ mail: null })
  useMobileNav.setState({ isOpen: false })
  api.GET.mockReset()
  api.GET.mockImplementation(async (path: string) => shellResponse(path))
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response('missing', { status: 404 })),
  )
})

const PLACES: { label: string; path: string }[] = [
  { label: 'Home', path: '/' },
  { label: 'Sprites', path: '/sprites' },
  { label: 'Memory', path: '/memory' },
  { label: 'Automations', path: '/automations' },
  { label: 'Coding', path: '/coding' },
  { label: 'Software', path: '/software' },
  { label: 'Settings', path: '/settings/connections' },
]

function places() {
  return screen.getByRole('navigation', { name: 'Places' })
}

it('keeps the closed mobile drawer out of keyboard and screen-reader navigation', async () => {
  setViewport(390)
  mount('/c/channel-1')
  const open = await screen.findByRole('button', { name: 'Open conversations' })
  expect(screen.queryByRole('navigation', { name: 'Places' })).toBeNull()
  fireEvent.click(open)
  expect(await screen.findByRole('navigation', { name: 'Places' })).toBeTruthy()
})

describe('the places', () => {
  it.each(PLACES)('navigates to $label and marks it current', async ({ label, path }) => {
    const history = mount('/c/channel-1')
    await screen.findByRole('heading', { name: 'Sage' })

    fireEvent.click(within(places()).getByRole('button', { name: label }))

    expect(history.location.pathname).toBe(path)
    await waitFor(() => {
      const item = within(places()).getByRole('button', { name: label })
      expect(item.getAttribute('aria-current')).toBe('page')
    })
  })

  // In a conversation the row holds the mark and no place does.
  it('marks the open conversation and no place', async () => {
    mount('/c/channel-1')
    await screen.findByRole('heading', { name: 'Sage' })

    const current = document.querySelectorAll('[aria-current="page"]')
    expect(current.length).toBe(1)
    expect(within(places()).queryByRole('button', { current: 'page' })).toBeNull()
  })

  it('closes the inspector when the user moves to a place', async () => {
    const history = mount('/c/channel-2')
    await screen.findByRole('heading', { name: 'Launch planning' })

    fireEvent.click(screen.getByRole('button', { name: 'Desk panel' }))
    expect(await screen.findByTestId('desk-panel')).toBeTruthy()

    fireEvent.click(within(places()).getByRole('button', { name: 'Automations' }))

    expect(await screen.findByRole('heading', { name: 'Automations' })).toBeTruthy()
    expect(screen.queryByTestId('desk-panel')).toBeNull()
    expect(history.location.href).not.toContain('panel=')
  })

  it('opens the command palette from the Search icon', async () => {
    mount('/')
    await screen.findByRole('navigation', { name: 'Places' })

    fireEvent.click(screen.getByRole('button', { name: 'Search' }))

    expect(
      await screen.findByLabelText('Search sprites, conversations, runs and commands'),
    ).toBeTruthy()
  })
})

describe('the conversation header toggles', () => {
  it('renders the Desk panel as a toggle', async () => {
    mount('/c/channel-2')
    await screen.findByRole('heading', { name: 'Launch planning' })

    const desk = screen.getByRole('button', { name: 'Desk panel' })
    expect(desk.getAttribute('aria-pressed')).toBe('false')

    fireEvent.click(desk)
    await screen.findByTestId('desk-panel')
    expect(
      screen.getByRole('button', { name: 'Desk panel' }).getAttribute('aria-pressed'),
    ).toBe('true')

    fireEvent.click(screen.getByRole('button', { name: 'Desk panel' }))
    await waitFor(() => expect(screen.queryByTestId('desk-panel')).toBeNull())
  })
})

describe('the mobile shell', () => {
  beforeEach(() => setViewport(375))

  it('returns to a conversation after opening Settings and changing sections', async () => {
    const history = mount('/c/channel-1')
    await screen.findByRole('heading', { name: 'Sage' })

    fireEvent.click(screen.getByRole('button', { name: 'Open conversations' }))
    fireEvent.click(within(places()).getByRole('button', { name: 'Settings' }))
    await screen.findByRole('heading', { name: 'Settings' })
    await waitFor(() => expect(screen.queryByRole('navigation', { name: 'Places' })).toBeNull())

    const settings = screen.getByRole('navigation', { name: 'Settings' })
    fireEvent.click(within(settings).getByRole('button', { name: 'Sound' }))
    await waitFor(() => expect(history.location.pathname).toBe('/settings/sound'))

    fireEvent.click(screen.getByRole('button', { name: 'Open conversations' }))
    expect(screen.getByRole('button', { name: 'Open conversations' }).getAttribute('aria-expanded')).toBe('true')
    fireEvent.click(within(screen.getByRole('navigation', { name: 'Conversations' })).getByRole('button', { name: /Sage/ }))

    await waitFor(() => expect(history.location.pathname).toBe('/c/channel-1'))
    await screen.findByRole('heading', { name: 'Sage' })
    await waitFor(() => expect(screen.queryByRole('navigation', { name: 'Places' })).toBeNull())
  })

  it.each(['/settings/sound', '/settings/connections/conn-1'])(
    'opens navigation from a direct Settings address: %s',
    async (url) => {
      const history = mount(url)
      fireEvent.click(await screen.findByRole('button', { name: 'Open conversations' }))
      fireEvent.click(within(places()).getByRole('button', { name: 'Home' }))

      await waitFor(() => expect(history.location.pathname).toBe('/'))
      await screen.findByTestId('home')
    },
  )

  // The drawer holds the whole sidebar: the places, the conversations
  // and the profile.
  it('reaches a place from the drawer', async () => {
    const history = mount('/c/channel-1')
    await screen.findByRole('heading', { name: 'Sage' })

    fireEvent.click(screen.getByRole('button', { name: 'Open conversations' }))
    fireEvent.click(within(places()).getByRole('button', { name: 'Software' }))

    expect(history.location.pathname).toBe('/software')
  })

  it('opens and closes the drawer with its own control', async () => {
    mount('/c/channel-1')
    await screen.findByRole('heading', { name: 'Sage' })

    fireEvent.click(screen.getByRole('button', { name: 'Open conversations' }))
    const close = screen.getByRole('button', { name: 'Close conversations' })
    fireEvent.click(close)

    expect(screen.queryByRole('button', { name: 'Close conversations' })).toBeNull()
  })

  it('opens the inspector as a sheet with a Back control', async () => {
    mount('/c/channel-2')
    await screen.findByRole('heading', { name: 'Launch planning' })

    fireEvent.click(screen.getByRole('button', { name: 'Desk panel' }))
    expect(await screen.findByTestId('desk-panel')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Back' }))
    await waitFor(() => expect(screen.queryByTestId('desk-panel')).toBeNull())
  })

  // A phone's slot is the whole screen, so a panel that opened by
  // itself would hide Home. The Desk panel toggle is the phone's way in.
  it('opens no Desk Panel by itself on a phone', async () => {
    mount('/')

    await screen.findByTestId('home')
    expect(screen.queryByTestId('desk-panel')).toBeNull()
  })

  it('opens the thread as a sheet with a Back control', async () => {
    const history = mount('/c/channel-2/t/message-1')
    expect(await screen.findByTestId('thread-pane')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Back' }))

    await waitFor(() => expect(screen.queryByTestId('thread-pane')).toBeNull())
    expect(history.location.pathname).toBe('/c/channel-2')
  })
})
