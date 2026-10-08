// The Coding Harnesses of one machine in Settings › Hosts: whether the
// machine can start each harness of the Harness Catalog, whether the
// daemon reports that it needs a sign-in there, and the Harness Sign-In
// that the Person starts.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, HostDto } from '../../api/client'
import { HostHarnesses } from './HostHarnesses'

const SIGN_IN_ROUTE = '/api/v1/hosts/{host_id}/harnesses/{harness_id}/sign-in'

const CATALOG = {
  items: [
    {
      id: 'claude',
      name: 'Claude Code',
      sign_in_methods: [
        { method: 'subscription', label: 'Subscription' },
        { method: 'api_key', label: 'API key' },
      ],
    },
    {
      id: 'codex',
      name: 'Codex',
      sign_in_methods: [
        { method: 'subscription', label: 'Subscription' },
        { method: 'api_key', label: 'API key' },
      ],
    },
  ],
}

function host(overrides: Partial<HostDto> = {}): HostDto {
  return {
    id: 'h-1',
    name: 'Air',
    platform: 'macos',
    capabilities: ['shell', 'harness:claude'],
    harnesses: [{ id: 'claude', needs_sign_in: false }],
    present: true,
    last_seen_at: 1_700_000_000_000,
    ...overrides,
  }
}

function mount(machine: HostDto, post: () => Promise<unknown> = async () => ({ data: { id: 's-1' } })) {
  const api = {
    GET: vi.fn(async () => ({ data: CATALOG })),
    POST: vi.fn(post),
  }
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <HostHarnesses api={api as unknown as ApiClient} host={machine} />
    </QueryClientProvider>,
  )
  return api
}

/** The row of one harness, which names it. */
async function harnessRow(name: string) {
  return within(await screen.findByRole('group', { name }))
}

describe('HostHarnesses', () => {
  it('says whether the machine can start each harness of the catalog', async () => {
    mount(host())

    expect((await harnessRow('Claude Code')).getByText('Can start')).toBeTruthy()
    expect((await harnessRow('Codex')).getByText('Not found on this computer')).toBeTruthy()
  })

  /** Pagis does not probe the sign-in state. The mark shows only what
   *  the daemon reports. */
  it('marks a harness that the daemon reports needs a sign-in on the machine', async () => {
    mount(
      host({
        capabilities: ['shell', 'harness:claude', 'harness:codex'],
        harnesses: [
          { id: 'claude', needs_sign_in: true },
          { id: 'codex', needs_sign_in: false },
        ],
      }),
    )

    expect((await harnessRow('Claude Code')).getByText('Needs sign-in')).toBeTruthy()
    expect((await harnessRow('Codex')).queryByText('Needs sign-in')).toBeNull()
  })

  it('gives one button for each sign-in method of a harness', async () => {
    mount(host())

    const claude = await harnessRow('Claude Code')
    expect(claude.getByRole('button', { name: 'Sign in with a subscription' })).toBeTruthy()
    expect(claude.getByRole('button', { name: 'Sign in with an API key' })).toBeTruthy()
  })

  it('starts the sign-in of the method on the machine once, and says where to finish it', async () => {
    const api = mount(host())

    const claude = await harnessRow('Claude Code')
    fireEvent.click(claude.getByRole('button', { name: 'Sign in with an API key' }))

    expect(
      await claude.findByText('A terminal window opened on Air. Finish the sign-in there.'),
    ).toBeTruthy()
    expect(api.POST).toHaveBeenCalledTimes(1)
    expect(api.POST).toHaveBeenCalledWith(SIGN_IN_ROUTE, {
      params: { path: { host_id: 'h-1', harness_id: 'claude' } },
      body: { method: 'api_key' },
    })
  })

  it('turns the buttons off on a machine that is not connected', async () => {
    mount(host({ present: false }))

    const claude = await harnessRow('Claude Code')
    for (const button of claude.getAllByRole('button')) {
      expect((button as HTMLButtonElement).disabled).toBe(true)
    }
  })

  it('turns the buttons off for a harness that the machine cannot start', async () => {
    mount(host())

    const codex = await harnessRow('Codex')
    for (const button of codex.getAllByRole('button')) {
      expect((button as HTMLButtonElement).disabled).toBe(true)
    }
    const claude = await harnessRow('Claude Code')
    for (const button of claude.getAllByRole('button')) {
      expect((button as HTMLButtonElement).disabled).toBe(false)
    }
  })

  it('shows the words of the daemon when the sign-in does not start', async () => {
    mount(host(), async () => ({
      error: { error: { code: 'not_connected', message: 'Air is not connected.' } },
    }))

    const claude = await harnessRow('Claude Code')
    fireEvent.click(claude.getByRole('button', { name: 'Sign in with a subscription' }))

    await waitFor(() =>
      expect(claude.getByRole('alert').textContent).toBe('Air is not connected.'),
    )
    expect(claude.queryByText(/A terminal window opened/)).toBeNull()
  })
})
