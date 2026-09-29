// The command palette: the shortcut opens it, a search jumps to
// the destination, the actions come after the matches and run their
// command, and the Dialog primitive traps the focus.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory, type RouterHistory } from '@tanstack/react-router'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { App } from '../App'
import { shellResponse } from '../test/appStub'

const { api } = vi.hoisted(() => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
}))

vi.mock('../api/client', async () => {
  const actual = await vi.importActual<typeof import('../api/client')>('../api/client')
  return { ...actual, createApiClient: () => api as unknown as ApiClient }
})

vi.mock('../ws/socket', () => ({
  PagisSocket: class {
    start() {}
    stop() {}
    subscribeChannel() {}
  },
}))

function mount(url = '/c/channel-1'): RouterHistory {
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

/** Open the palette with the shortcut and wait for the list. */
async function openPalette() {
  fireEvent.keyDown(window, { key: 'k', metaKey: true })
  return await screen.findByRole('listbox', { name: 'Results' })
}

function search(): HTMLElement {
  return screen.getByRole('textbox', {
    name: 'Search sprites, conversations, runs and commands',
  })
}

beforeEach(() => {
  vi.stubGlobal(
    'matchMedia',
    (query: string) => ({
      matches: false,
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      dispatchEvent: () => false,
    }),
  )
  document.documentElement.removeAttribute('data-theme')
  window.localStorage.clear()
  api.GET.mockReset()
  api.POST.mockReset()
  api.GET.mockImplementation(async (path: string) => {
    if (path === '/api/v1/runs') {
      return {
        data: {
          items: [
            {
              id: 'run-1',
              agent_id: 'agent-1',
              state: 'completed',
              trigger_kind: 'message',
              hop_count: 0,
              created_at: 1,
            },
          ],
        },
      }
    }
    return shellResponse(path)
  })
  api.POST.mockImplementation(async () => ({ data: {} }))
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response('missing', { status: 404 })),
  )
})

describe('the command palette', () => {
  it('opens with the shortcut, finds a channel and navigates on Enter', async () => {
    const history = mount()
    await screen.findByRole('heading', { name: 'Sage' })

    await openPalette()
    fireEvent.change(search(), { target: { value: 'Launch' } })

    const list = screen.getByRole('listbox', { name: 'Results' })
    await waitFor(() =>
      expect(within(list).getAllByRole('option')).toHaveLength(1),
    )
    expect(within(list).getByRole('option').textContent).toContain(
      'Launch planning',
    )

    fireEvent.keyDown(search(), { key: 'Enter' })

    expect(history.location.pathname).toBe('/c/channel-2')
    await waitFor(() =>
      expect(screen.queryByRole('listbox', { name: 'Results' })).toBeNull(),
    )
  })

  it('closes on Escape', async () => {
    mount()
    await screen.findByRole('heading', { name: 'Sage' })

    await openPalette()
    fireEvent.keyDown(search(), { key: 'Escape' })

    await waitFor(() =>
      expect(screen.queryByRole('listbox', { name: 'Results' })).toBeNull(),
    )
  })

  it('searches sprites, runs and settings sections', async () => {
    const history = mount()
    await screen.findByRole('heading', { name: 'Sage' })

    await openPalette()
    const list = screen.getByRole('listbox', { name: 'Results' })
    const labels = () =>
      within(list)
        .getAllByRole('option')
        .map((option) => option.textContent ?? '')

    await waitFor(() =>
      expect(labels().some((label) => label.startsWith('Sage · completed'))).toBe(
        true,
      ),
    )
    expect(labels().some((label) => label.includes('Retention'))).toBe(true)
    expect(labels().some((label) => label.includes('Sprite'))).toBe(true)

    fireEvent.change(search(), { target: { value: 'Retention' } })
    fireEvent.keyDown(search(), { key: 'Enter' })
    expect(history.location.pathname).toBe('/settings/retention')
  })

  // The matches come first; the commands come after them.
  it('lists the actions after the matches', async () => {
    mount()
    await screen.findByRole('heading', { name: 'Sage' })

    const list = await openPalette()
    await waitFor(() =>
      expect(
        within(list)
          .getAllByRole('option')
          .some((option) => option.textContent?.includes('Hire a sprite')),
      ).toBe(true),
    )

    const options = within(list).getAllByRole('option')
    const firstAction = options.findIndex((option) =>
      option.textContent?.includes('Hire a sprite'),
    )
    const lastMatch = options.findIndex((option) =>
      option.textContent?.includes('Launch planning'),
    )
    expect(lastMatch).toBeGreaterThanOrEqual(0)
    expect(firstAction).toBeGreaterThan(lastMatch)
  })

  it('runs Hire a sprite', async () => {
    const history = mount()
    await screen.findByRole('heading', { name: 'Sage' })

    await openPalette()
    fireEvent.change(search(), { target: { value: 'Hire a sprite' } })
    fireEvent.keyDown(search(), { key: 'Enter' })

    expect(history.location.pathname).toBe('/sprites')
    expect(history.location.search).toContain('new=1')
  })

  it('runs Open Runs', async () => {
    const history = mount()
    await screen.findByRole('heading', { name: 'Sage' })

    await openPalette()
    fireEvent.change(search(), { target: { value: 'Open Runs' } })
    fireEvent.keyDown(search(), { key: 'Enter' })

    expect(history.location.pathname).toBe('/runs')
  })

  it('wakes an agent computer', async () => {
    mount()
    await screen.findByRole('heading', { name: 'Sage' })

    await openPalette()
    fireEvent.change(search(), { target: { value: 'Wake computer' } })
    await waitFor(() =>
      expect(
        within(screen.getByRole('listbox', { name: 'Results' })).getAllByRole(
          'option',
        ),
      ).toHaveLength(1),
    )
    fireEvent.keyDown(search(), { key: 'Enter' })

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/computer/wake',
        { params: { path: { agent_id: 'agent-1' } } },
      ),
    )
  })

  it('switches away from the system dark theme on the first toggle', async () => {
    const matchMedia = window.matchMedia
    vi.stubGlobal('matchMedia', (query: string) => ({
      ...matchMedia(query),
      matches: query === '(prefers-color-scheme: dark)',
    }))
    mount()
    await screen.findByRole('heading', { name: 'Sage' })
    await openPalette()
    fireEvent.change(search(), { target: { value: 'Toggle theme' } })
    fireEvent.keyDown(search(), { key: 'Enter' })
    expect(document.documentElement.getAttribute('data-theme')).toBe('light')
    expect(window.localStorage.getItem('pagis-theme')).toBe('light')
  })

  it('toggles the theme and remembers it', async () => {
    mount()
    await screen.findByRole('heading', { name: 'Sage' })

    await openPalette()
    fireEvent.change(search(), { target: { value: 'Toggle theme' } })
    fireEvent.keyDown(search(), { key: 'Enter' })

    expect(document.documentElement.getAttribute('data-theme')).toBe('dark')
    expect(window.localStorage.getItem('pagis-theme')).toBe('dark')
  })

  it('moves the selection with the arrow keys', async () => {
    const history = mount()
    await screen.findByRole('heading', { name: 'Sage' })

    const list = await openPalette()
    fireEvent.change(search(), { target: { value: 'Conversation' } })
    // Both conversations match on the hint.
    await waitFor(() =>
      expect(within(list).getAllByRole('option').length).toBeGreaterThan(1),
    )

    const first = within(list).getAllByRole('option')[0]
    expect(first?.getAttribute('aria-selected')).toBe('true')

    fireEvent.keyDown(search(), { key: 'ArrowDown' })
    await waitFor(() =>
      expect(
        within(screen.getByRole('listbox', { name: 'Results' }))
          .getAllByRole('option')[1]
          ?.getAttribute('aria-selected'),
      ).toBe('true'),
    )

    fireEvent.keyDown(search(), { key: 'Enter' })
    expect(history.location.pathname).toBe('/c/channel-2')
  })

  // The Dialog primitive owns the focus trap: the palette is modal and
  // the shell behind it is hidden from the accessibility tree.
  it('traps the focus in a modal dialog', async () => {
    mount()
    await screen.findByRole('heading', { name: 'Sage' })

    const user = userEvent.setup()
    await openPalette()
    const dialog = screen.getByRole('dialog')
    expect(dialog.contains(document.activeElement)).toBe(true)

    // Tab walks the palette and never leaves it.
    for (let step = 0; step < 6; step += 1) await user.tab()
    expect(dialog.contains(document.activeElement)).toBe(true)
  })
})
