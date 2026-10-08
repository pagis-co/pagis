// One sidebar, one behaviour, and the mobile shell.
// Every place navigates and says it is current; a move to a place puts
// the inspector away. The phone has four tabs and pushed screens.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory, type RouterHistory } from '@tanstack/react-router'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from './api/client'
import { App } from './App'
import { useCallInspector, useMailInspector } from './state/stores'
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
  { label: 'Software', path: '/software' },
  { label: 'Settings', path: '/settings/connections' },
]

function places() {
  return screen.getByRole('navigation', { name: 'Places' })
}

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

describe('the phone shell', () => {
  beforeEach(() => setViewport(390))
  it('opens all four places and marks the current tab', async () => {
    const history = mount('/')
    await screen.findByTestId('home')
    const nav = places()
    expect(within(nav).getAllByRole('link').map((link) => link.getAttribute('aria-label'))).toEqual(['Home', 'Conversations', 'Sprites', 'You'])
    expect(within(nav).getByRole('link', { name: 'Home' }).getAttribute('aria-current')).toBe('page')
    fireEvent.click(within(nav).getByRole('link', { name: 'Sprites' }))
    await waitFor(() => expect(history.location.pathname).toBe('/sprites'))
    expect(within(places()).getByRole('link', { name: 'Sprites' }).getAttribute('aria-current')).toBe('page')
  })
  it('uses a fixed parent and no tabs in a conversation', async () => {
    const history = mount('/c/channel-1')
    await screen.findByRole('heading', { name: 'Sage' })
    expect(screen.queryByRole('navigation', { name: 'Places' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Conversations' }))
    await waitFor(() => expect(history.location.pathname).toBe('/conversations'))
  })
  it('opens Settings as three groups without Administration', async () => {
    mount('/settings')
    await screen.findByRole('heading', { name: 'Settings' })
    expect(screen.queryByText('Administration')).toBeNull()
    expect(screen.getByText('Access')).toBeTruthy()
    expect(screen.getByText('Models', { selector: '.ui-section-label' })).toBeTruthy()
    expect(screen.getByText('System')).toBeTruthy()
  })
  it('opens no Desk Panel by itself', async () => {
    mount('/')
    await screen.findByTestId('home')
    expect(screen.queryByTestId('desk-panel')).toBeNull()
  })
})
