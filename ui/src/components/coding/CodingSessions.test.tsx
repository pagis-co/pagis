// `/coding` lists every Coding Session of the Workspace: the open ones
// first, with the ones that need the Person at the top, then the ended
// ones. A row opens the session page.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, CodingSessionDto } from '../../api/client'
import { codingSession, shellResponse } from '../../test/appStub'
import { formatMoment } from '../../timeline'
import { CodingSessions } from './CodingSessions'

const HOUR = 3_600_000
const NOW = Date.now()

function session(overrides: Partial<CodingSessionDto>): CodingSessionDto {
  return { ...(codingSession as unknown as CodingSessionDto), ...overrides }
}

/** A session that waits for the Person on a Harness Permission. */
const waitsForYou = session({
  id: 'session-you',
  state: 'needs_decision',
  directory: '/Users/ada/src/you',
  updated_at: NOW - 3 * HOUR,
  pending: { kind: 'permission', waits_for: 'person', seq: 4 },
})

/** A session whose Agent decides the permission. */
const waitsForSage = session({
  id: 'session-sage',
  state: 'needs_decision',
  directory: '/Users/ada/src/sage',
  updated_at: NOW - 2 * HOUR,
  pending: { kind: 'permission', waits_for: 'agent', seq: 7 },
})

const working = session({
  id: 'session-working',
  title: 'Fix the token check',
  state: 'working',
  directory: '/Users/ada/src/working',
  updated_at: NOW - HOUR,
})

const closed = session({
  id: 'session-closed',
  state: 'closed',
  directory: '/Users/ada/src/closed',
  updated_at: NOW - 5 * HOUR,
  ended_at: NOW - 5 * HOUR,
  end_reason: 'closed',
})

const failed = session({
  id: 'session-failed',
  state: 'failed',
  directory: '/Users/ada/src/failed',
  updated_at: NOW - 4 * HOUR,
  ended_at: NOW - 4 * HOUR,
  end_reason: 'harness_exited',
})

/** The api of the shell stub, with `pages` as the pages of the list
 *  route, one for each `before`. */
function stubApi(pages: Record<string, CodingSessionDto[]>) {
  return {
    GET: vi.fn(async (path: string, init?: { params?: { query?: { before?: string } } }) => {
      if (path === '/api/v1/coding-sessions') {
        const before = init?.params?.query?.before ?? ''
        return { data: { items: pages[before] ?? [] } }
      }
      return shellResponse(path)
    }),
  } as unknown as ApiClient & { GET: ReturnType<typeof vi.fn> }
}

function mount(api: ApiClient) {
  const onOpenSession = vi.fn()
  const onOpenChannel = vi.fn()
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <CodingSessions
        api={api}
        onOpenSession={onOpenSession}
        onOpenChannel={onOpenChannel}
      />
    </QueryClientProvider>,
  )
  return { onOpenSession, onOpenChannel }
}

/** The directories of the rows of one section, in order. */
function directories(name: string): string[] {
  const section = screen.getByRole('region', { name })
  return within(section)
    .getAllByRole('button')
    .map((row) => row.querySelector('.coding-list-directory')?.textContent ?? '')
}

describe('the list of coding sessions', () => {
  it('puts the open sessions before the ended ones', async () => {
    mount(stubApi({ '': [working, closed, failed] }))

    expect(await screen.findByRole('heading', { name: 'Coding' })).toBeTruthy()
    const open = await screen.findByRole('region', { name: 'Open' })
    const ended = screen.getByRole('region', { name: 'Ended' })
    expect(open.compareDocumentPosition(ended) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(directories('Open')).toEqual(['/Users/ada/src/working'])
    // The newest end first.
    expect(directories('Ended')).toEqual(['/Users/ada/src/failed', '/Users/ada/src/closed'])
  })

  it('puts a session that needs you first in Open, with the mark', async () => {
    mount(stubApi({ '': [working, waitsForSage, waitsForYou] }))

    await screen.findByRole('region', { name: 'Open' })
    expect(directories('Open')).toEqual([
      '/Users/ada/src/you',
      '/Users/ada/src/working',
      '/Users/ada/src/sage',
    ])
    const rows = within(screen.getByRole('region', { name: 'Open' })).getAllByRole('button')
    expect(within(rows[0]).getByText('Needs you')).toBeTruthy()
  })

  it('has no mark on a session that waits for its sprite', async () => {
    mount(stubApi({ '': [waitsForSage] }))

    const open = await screen.findByRole('region', { name: 'Open' })
    const [row] = within(open).getAllByRole('button')
    expect(within(row).getByText('Waits for a decision')).toBeTruthy()
    expect(within(row).queryByText('Needs you')).toBeNull()
  })

  it('shows the title, the state, the sprite, the harness, the machine, the directory and the time', async () => {
    mount(stubApi({ '': [working] }))

    const open = await screen.findByRole('region', { name: 'Open' })
    const [row] = within(open).getAllByRole('button')
    // The title leads the row, and the other facts follow it.
    const title = within(row).getByText('Fix the token check')
    const facts = within(row).getByText('Claude Code')
    expect(title.compareDocumentPosition(facts) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(within(row).getByText('Running')).toBeTruthy()
    expect(await within(row).findByText('Sage')).toBeTruthy()
    expect(within(row).getByText('Claude Code')).toBeTruthy()
    expect(within(row).getByText('Ada’s laptop')).toBeTruthy()
    expect(within(row).getByText('/Users/ada/src/working')).toBeTruthy()
    expect(within(row).getByText(formatMoment(working.updated_at))).toBeTruthy()
  })

  it('shows the Harness Mode after the harness', async () => {
    mount(stubApi({ '': [session({ ...working, harness_mode_name: 'Plan' })] }))

    const open = await screen.findByRole('region', { name: 'Open' })
    const [row] = within(open).getAllByRole('button')
    const harness = within(row).getByText('Claude Code')
    const mode = within(row).getByText('Plan')
    expect(harness.compareDocumentPosition(mode) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(
      mode.compareDocumentPosition(within(row).getByText('Ada’s laptop')) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy()
  })

  it('shows no Harness Mode for a session with none', async () => {
    mount(stubApi({ '': [session({ ...working, harness_mode_name: null })] }))

    const open = await screen.findByRole('region', { name: 'Open' })
    const [row] = within(open).getAllByRole('button')
    const facts = row.querySelector('.coding-list-facts')
    await within(row).findByText('Sage')
    expect(facts?.textContent).toBe('Sage · Claude Code · Ada’s laptop · /Users/ada/src/working')
  })

  it('opens the session of a row', async () => {
    const { onOpenSession } = mount(stubApi({ '': [working] }))

    const open = await screen.findByRole('region', { name: 'Open' })
    fireEvent.click(within(open).getByRole('button'))

    expect(onOpenSession).toHaveBeenCalledWith('session-working')
  })

  it('reads each page of the list', async () => {
    const first = Array.from({ length: 100 }, (_, index) =>
      session({ id: `session-${200 - index}`, state: 'closed', updated_at: NOW - index }),
    )
    const api = stubApi({ '': first, 'session-101': [working] })
    mount(api)

    await waitFor(() => expect(directories('Open')).toEqual(['/Users/ada/src/working']))
    expect(api.GET).toHaveBeenCalledWith('/api/v1/coding-sessions', {
      params: { query: { before: 'session-101', limit: 100 } },
    })
  })

  it('asks the Chief of Staff from the empty list', async () => {
    const { onOpenChannel } = mount(stubApi({}))

    expect(
      await screen.findByText(
        'No coding session yet. A sprite starts one when you ask it to change code.',
      ),
    ).toBeTruthy()
    fireEvent.click(await screen.findByRole('button', { name: 'Ask Sage' }))

    expect(onOpenChannel).toHaveBeenCalledWith('channel-1')
  })
})
