// The You tab: the places under the Person, the switches of this
// phone, and the confirmations of Sign out and Change server.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { Capacitor } from '@capacitor/core'
import userEvent from '@testing-library/user-event'
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { components } from '../../api/schema'
import type { ApiClient } from '../../api/client'
import { renderInRouter } from '../../test/router'
import { formatClock } from '../../timeline'
import { You } from './You'

const { shell } = vi.hoisted(() => ({
  shell: {
    getLockScreenAnswers: vi.fn(),
    setLockScreenAnswers: vi.fn(),
    changeServer: vi.fn(),
  },
}))
vi.mock('../../mobileShell', async () => ({
  ...(await vi.importActual('../../mobileShell')),
  PagisShell: shell,
}))

const DAY = 86_400_000

function stubApi(schedules: unknown[] = [], links: { label: string; url: string }[] = [], entries: { label: string; url: string; detail?: string; current: boolean }[] = [], notice: components['schemas']['PersonNoticeDto'] | null = null) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/user') return { data: { notice, org: { name: 'Acme', entries }, role: 'member', id: 'u1', name: 'Ajay', email: 'a@example.com', product_app_links: links } }
      if (path === '/api/v1/schedules') return { data: { items: schedules } }
      return { data: { items: [] } }
    }),
    POST: vi.fn(),
    PUT: vi.fn(),
    DELETE: vi.fn(async () => ({ response: new Response(null, { status: 204 }) })),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return renderInRouter(
    <QueryClientProvider client={client}>
      <You api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

beforeEach(() => {
  shell.getLockScreenAnswers.mockReset().mockResolvedValue({ on: true })
  shell.setLockScreenAnswers.mockReset().mockResolvedValue(undefined)
  shell.changeServer.mockReset().mockResolvedValue(undefined)
})

afterEach(() => {
  vi.restoreAllMocks()
})

describe('in a browser', () => {
  it('lists the places, with the next Schedule on the clock of the app', async () => {
    const due = new Date(Date.now() + DAY)
    due.setHours(7, 30, 0, 0)
    mount(
      stubApi([
        { id: 'sc1', name: "Pixie's brief", state: 'active', next_due_at: due.getTime() },
      ]),
    )
    expect(await screen.findByRole('heading', { name: 'Ajay' })).not.toBeNull()
    expect(
      await screen.findByText(`Next: Pixie's brief, tomorrow ${formatClock(due.getTime())}`),
    ).not.toBeNull()
    for (const place of ['Memory', 'Automations', 'Software', 'Settings']) {
      expect(screen.getByRole('button', { name: new RegExp(`^${place}`) })).not.toBeNull()
    }
    expect(screen.queryByText('Answer on the lock screen')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Change server' })).toBeNull()
  })

  it('opens a place', async () => {
    const history = mount(stubApi())
    fireEvent.click(await screen.findByRole('button', { name: /^Settings/ }))
    await waitFor(() => expect(history.location.pathname).toBe('/settings'))
  })

  it('confirms Sign out before it signs out', async () => {
    const api = stubApi()
    mount(api)
    fireEvent.click(await screen.findByRole('button', { name: 'Sign out' }))
    const sheet = screen.getByRole('alertdialog', { name: 'Sign out of Pagis?' })
    expect(api.DELETE).not.toHaveBeenCalled()
    fireEvent.click(within(sheet).getByRole('button', { name: 'Sign out' }))
    await waitFor(() => expect(api.DELETE).toHaveBeenCalledWith('/api/v1/sessions/current'))
  })
})

describe('in the Mobile App', () => {
  beforeEach(() => {
    vi.spyOn(Capacitor, 'isNativePlatform').mockReturnValue(true)
  })

  it('reads and sets the lock-screen answers through the bridge', async () => {
    mount(stubApi())
    const row = await screen.findByRole('switch', { name: /Answer on the lock screen/ })
    await waitFor(() => expect(row.getAttribute('aria-checked')).toBe('true'))
    fireEvent.click(row)
    await waitFor(() => expect(shell.setLockScreenAnswers).toHaveBeenCalledWith({ on: false }))
    await waitFor(() => expect(row.getAttribute('aria-checked')).toBe('false'))
  })

  it('confirms Change server, names the session it leaves, and changes it', async () => {
    mount(stubApi())
    fireEvent.click(await screen.findByRole('button', { name: 'Change server' }))
    const sheet = screen.getByRole('alertdialog', { name: 'Change server?' })
    expect(sheet.textContent).toContain('until you remove it there')
    expect(shell.changeServer).not.toHaveBeenCalled()
    fireEvent.click(within(sheet).getByRole('button', { name: 'Change server' }))
    await waitFor(() => expect(shell.changeServer).toHaveBeenCalled())
  })
})

describe('the Server links in You', () => {
  it('shows each link in the same tab', async () => {
    const links = [
      { label: 'Servers', url: 'https://team.example.com/servers' },
      { label: 'Help', url: 'https://team.example.com/help' },
      { label: 'Account', url: 'https://team.example.com/account' },
    ]
    mount(stubApi([], links))
    const menu = await screen.findByRole('navigation', { name: 'Server links' })
    for (const link of links) {
      const entry = within(menu).getByRole('link', { name: link.label })
      expect(entry.getAttribute('href')).toBe(link.url)
      expect(entry.getAttribute('target')).toBeNull()
    }
  })

  it('shows no Server links when the list is empty', async () => {
    mount(stubApi())
    await screen.findByRole('heading', { name: 'Ajay' })
    expect(screen.queryByRole('navigation', { name: 'Server links' })).toBeNull()
    expect(screen.queryByRole('link')).toBeNull()
  })
})

it('shows the Org in You without a button when entries are empty', async () => {
  mount(stubApi())
  const name = await screen.findByText('Acme')
  expect(screen.getByText('Member')).toBeTruthy()
  expect(name.closest('button')).toBeNull()
})
it('opens the same Org entries in You', async () => {
  const user = userEvent.setup()
  mount(stubApi([], [], [{ label: 'Acme', url: 'https://acme.example.com', detail: 'Member', current: true }]))
  await user.click(await screen.findByRole('button', { name: 'Acme Member' }))
  const entry = screen.getByRole('menuitem', { name: 'Acme Member' })
  expect(entry.getAttribute('href')).toBe('https://acme.example.com')
  expect(entry.getAttribute('aria-current')).toBe('true')
  expect(entry.getAttribute('target')).toBeNull()
})

describe('the Person Notice', () => {
  it.each([null, 'neutral', 'waiting'] as const)('shows the notice with tone %s', async (tone) => {
    const notice = { tone, label: 'Balance', text: 'Your balance is low.', link: { label: 'Add funds', url: 'https://org.example.com/balance' } }
    mount(stubApi([], [], [], notice))
    const note = await screen.findByRole('note')
    expect(within(note).getByText('Balance').className).toContain(`ui-badge-${tone ?? 'neutral'}`)
    expect(within(note).getByText(notice.text)).toBeTruthy()
    const link = within(note).getByRole('link', { name: 'Add funds' })
    expect(link.getAttribute('href')).toBe(notice.link.url)
    expect(link.getAttribute('target')).toBeNull()
    expect(note.parentElement?.firstElementChild).toBe(note)
  })
  it('shows nothing when the server supplies no notice', async () => {
    mount(stubApi())
    await screen.findByText('Acme')
    expect(screen.queryByRole('note')).toBeNull()
  })
  it('shows no link when the notice has none', async () => {
    const notice = { tone: null, label: 'Balance', text: 'Your balance is low.', link: null }
    mount(stubApi([], [], [], notice))
    const note = await screen.findByRole('note')
    expect(within(note).queryByRole('link')).toBeNull()
  })
})
