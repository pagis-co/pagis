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
    present: true,
    last_seen_at: 1_700_000_000_000,
    ...overrides,
  }
}

function mount(items: ReturnType<typeof host>[]) {
  const api = { GET: vi.fn(async () => ({ data: { items } })) }
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

  /** A browser-only person has no machine, and the copy says what to do
   *  about it. */
  it('tells a person with no machine to open the client', async () => {
    mount([])

    await waitFor(() =>
      expect(screen.getByText(/Open the Pagis client on a computer/)).toBeTruthy(),
    )
  })
})
