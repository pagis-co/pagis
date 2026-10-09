// The Hosts section: the person's own machines and which of them
// a sprite can act on right now.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { Hosts } from './Hosts'

function host(overrides: Record<string, unknown> = {}) {
  return {
    id: 'h-1',
    name: 'Air',
    platform: 'macos',
    capabilities: ['shell'],
    harnesses: [],
    present: true,
    last_seen_at: 1_700_000_000_000,
    ...overrides,
  }
}

const CATALOG = {
  items: [{ id: 'claude', name: 'Claude Code', sign_in_methods: [], checks_sign_in: true }],
}

function mount(items: ReturnType<typeof host>[]) {
  const api = {
    GET: vi.fn(async (path: string) => ({
      data: path === '/api/v1/harnesses' ? CATALOG : { items },
    })),
  }
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={queryClient}>
      <Hosts api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
  return api
}

describe('Hosts', () => {
  it('names each machine, its system and whether it is connected', async () => {
    mount([host(), host({ id: 'h-2', name: 'Studio', present: false })])

    await waitFor(() => expect(screen.getAllByTestId('host-row')).toHaveLength(2))
    expect(screen.getByText('Air')).toBeTruthy()
    expect(screen.getAllByText('macOS')).toHaveLength(2)
    expect(screen.getByText('Connected')).toBeTruthy()
    expect(screen.getByText('Not connected')).toBeTruthy()
    expect(screen.getByText('1 of 2 connected')).toBeTruthy()
  })

  /** A machine that is not connected says when it was last here, which is
   *  the useful thing to say about one that runs nothing now. */
  it('says when an absent machine was last seen', async () => {
    mount([host({ present: false })])

    await waitFor(() => expect(screen.getByText('Not connected')).toBeTruthy())
    expect(screen.getByText(/last seen/)).toBeTruthy()
  })

  /** A phone registers with no shell, so the row says it runs no command
   *  rather than leaving the person to guess. */
  it('says a machine that runs no command runs none', async () => {
    mount([host({ name: 'Phone', platform: 'ios', capabilities: [] })])

    await waitFor(() => expect(screen.getByText('No commands')).toBeTruthy())
    expect(screen.getByText('iOS')).toBeTruthy()
  })

  /** A phone runs no harness, so its row lists none. */
  it('lists no harness under a machine that runs no command', async () => {
    mount([
      host({ name: 'Phone', platform: 'ios', capabilities: [] }),
      host({ id: 'h-2', name: 'Studio' }),
    ])

    expect(await screen.findByRole('group', { name: 'Claude Code' })).toBeTruthy()
    expect(screen.getAllByRole('group', { name: 'Claude Code' })).toHaveLength(1)
  })

  /** The Vault add form stays the only place a person types a secret
   *  (ADR-0022), and the hint says so for a Harness Sign-In. */
  it('says that Pagis never sees the credential of a sign-in', async () => {
    mount([host()])

    expect(
      await screen.findByText(/Pagis never sees your password or your key\./),
    ).toBeTruthy()
  })

  /** A browser-only person has no machine, and the copy says what to do
   *  about it. */
  it('tells a person with no machine to open the client', async () => {
    mount([])

    await waitFor(() =>
      expect(screen.getByText(/Open the Pagis client on a computer/)).toBeTruthy(),
    )
  })
})
