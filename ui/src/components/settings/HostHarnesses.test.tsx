// The Coding Harnesses of one machine in Settings › Hosts: whether the
// machine can start each harness of the Harness Catalog, its sign-in
// state there, the Harness Sign-In that the Person starts, and how that
// sign-in ended.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, HostDto } from '../../api/client'
import { HostHarnesses } from './HostHarnesses'

const SIGN_IN_ROUTE = '/api/v1/hosts/{host_id}/harnesses/{harness_id}/sign-in'
const CHECK_ROUTE = '/api/v1/hosts/{host_id}/harnesses/{harness_id}/sign-in-check'

const CATALOG = {
  items: [
    {
      id: 'claude',
      name: 'Claude Code',
      sign_in_methods: [
        { method: 'subscription', label: 'Subscription' },
        { method: 'api_key', label: 'API key' },
      ],
      checks_sign_in: true,
    },
    {
      id: 'codex',
      name: 'Codex',
      sign_in_methods: [
        { method: 'subscription', label: 'Subscription' },
        { method: 'api_key', label: 'API key' },
      ],
      checks_sign_in: true,
    },
    {
      id: 'gemini',
      name: 'Gemini CLI',
      sign_in_methods: [{ method: 'subscription', label: 'Subscription' }],
      checks_sign_in: false,
    },
  ],
}

type HostHarness = HostDto['harnesses'][number]

function harness(id: string, overrides: Partial<HostHarness> = {}): HostHarness {
  return { id, needs_sign_in: false, sign_in_state: 'unknown', last_sign_in: null, ...overrides }
}

function host(overrides: Partial<HostDto> = {}): HostDto {
  return {
    id: 'h-1',
    name: 'Air',
    platform: 'macos',
    capabilities: ['shell', 'harness:claude'],
    harnesses: [harness('claude')],
    present: true,
    last_seen_at: 1_700_000_000_000,
    ...overrides,
  }
}

const BOTH = ['shell', 'harness:claude', 'harness:codex']

/** The answers of the routes: the sign-in answers its id, and the check
 *  answers 204 with no body. */
async function answer(route: string): Promise<unknown> {
  if (route === CHECK_ROUTE) return { response: new Response(null, { status: 204 }) }
  return { data: { id: 's-1' } }
}

function mount(machine: HostDto, post: (route: string) => Promise<unknown> = answer) {
  const api = {
    GET: vi.fn(async () => ({ data: CATALOG })),
    POST: vi.fn(post),
  }
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  const view = (next: HostDto) => (
    <QueryClientProvider client={queryClient}>
      <HostHarnesses api={api as unknown as ApiClient} host={next} />
    </QueryClientProvider>
  )
  const { rerender } = render(view(machine))
  return { api, rerender: (next: HostDto) => rerender(view(next)) }
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

  it.each([
    [{ sign_in_state: 'signed_in' }, 'Signed in'],
    [{ sign_in_state: 'not_signed_in' }, 'Not signed in'],
    [{ needs_sign_in: true }, 'Needs sign-in'],
    [{ needs_sign_in: true, sign_in_state: 'not_signed_in' }, 'Not signed in'],
    // The status command found a credential, and the harness refused it.
    [{ needs_sign_in: true, sign_in_state: 'signed_in' }, 'Sign-in expired'],
  ] as const)('marks the report %j as %s', async (report, mark) => {
    mount(host({ capabilities: BOTH, harnesses: [harness('claude', report), harness('codex')] }))

    expect((await harnessRow('Claude Code')).getByText(mark)).toBeTruthy()
  })

  it('shows no mark for a harness whose sign-in state is unknown', async () => {
    mount(host({ capabilities: BOTH, harnesses: [harness('claude'), harness('codex')] }))

    const codex = await harnessRow('Codex')
    for (const mark of ['Signed in', 'Not signed in', 'Needs sign-in', 'Sign-in expired']) {
      expect(codex.queryByText(mark)).toBeNull()
    }
  })

  it('tells the Person to sign in again when the sign-in expired', async () => {
    mount(host({ harnesses: [harness('claude', { needs_sign_in: true, sign_in_state: 'signed_in' })] }))

    expect(
      (await harnessRow('Claude Code')).getByText(
        'Claude Code refused a session on Air. The sign-in expired or was revoked. Sign in again.',
      ),
    ).toBeTruthy()
  })

  it('gives one button for each sign-in method of a harness', async () => {
    mount(host())

    const claude = await harnessRow('Claude Code')
    expect(claude.getByRole('button', { name: 'Sign in with a subscription' })).toBeTruthy()
    expect(claude.getByRole('button', { name: 'Sign in with an API key' })).toBeTruthy()
  })

  it('checks the sign-in on the machine when the Person asks', async () => {
    const { api } = mount(host())

    const claude = await harnessRow('Claude Code')
    fireEvent.click(claude.getByRole('button', { name: 'Check sign-in' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(CHECK_ROUTE, {
        params: { path: { host_id: 'h-1', harness_id: 'claude' } },
      }),
    )
  })

  it('offers no check for a harness with no status command', async () => {
    mount(host({ capabilities: ['shell', 'harness:gemini'], harnesses: [harness('gemini')] }))

    expect((await harnessRow('Gemini CLI')).queryByRole('button', { name: 'Check sign-in' })).toBeNull()
  })

  it('shows the words of the daemon when the check does not reach the machine', async () => {
    mount(host(), async () => ({
      error: { error: { code: 'host_not_connected', message: 'Air is not connected.' } },
      response: new Response(null, { status: 409 }),
    }))

    const claude = await harnessRow('Claude Code')
    fireEvent.click(claude.getByRole('button', { name: 'Check sign-in' }))

    await waitFor(() => expect(claude.getByRole('alert').textContent).toBe('Air is not connected.'))
  })

  it('starts the sign-in of the method on the machine once, and says where to finish it', async () => {
    const { api } = mount(host())

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

  it.each([
    [{ exit_code: 0 }, 'signed_in', 'You are signed in to Claude Code on Air.'],
    [{ exit_code: 0 }, 'not_signed_in', 'The sign-in ended, and Claude Code is still not signed in on Air.'],
    [{ exit_code: 0 }, 'unknown', 'The sign-in ended. The next session tells whether it worked.'],
    [{ exit_code: 130 }, 'unknown', 'The sign-in ended with exit code 130. Sign in again.'],
    [
      { exit_code: null, error: 'The sign-in did not end in 30 minutes' },
      'unknown',
      'The sign-in did not end in 30 minutes. Sign in again.',
    ],
    [
      { exit_code: null, error: 'the machine disconnected before the sign-in ended' },
      'unknown',
      'The machine disconnected before the sign-in ended. Sign in again.',
    ],
  ] as const)('updates the hint when the terminal window closes: %j and %s', async (end, state, hint) => {
    const machine = host()
    const { rerender } = mount(machine)
    const claude = await harnessRow('Claude Code')
    fireEvent.click(claude.getByRole('button', { name: 'Sign in with a subscription' }))
    await claude.findByText('A terminal window opened on Air. Finish the sign-in there.')

    rerender({
      ...machine,
      harnesses: [
        harness('claude', {
          sign_in_state: state,
          last_sign_in: { id: 's-1', running: false, error: null, ...end },
        }),
      ],
    })

    expect(await claude.findByText(hint)).toBeTruthy()
    expect(claude.queryByText(/A terminal window opened/)).toBeNull()
  })

  it('keeps the hint while the sign-in runs, and shows no end of a sign-in that this page did not start', async () => {
    const machine = host({
      harnesses: [harness('claude', { last_sign_in: { id: 'older', running: false, exit_code: 1, error: null } })],
    })
    const { rerender } = mount(machine)
    const claude = await harnessRow('Claude Code')
    expect(claude.queryByText(/The sign-in ended/)).toBeNull()

    fireEvent.click(claude.getByRole('button', { name: 'Sign in with a subscription' }))
    await claude.findByText('A terminal window opened on Air. Finish the sign-in there.')
    rerender({
      ...machine,
      harnesses: [harness('claude', { last_sign_in: { id: 's-1', running: true, exit_code: null, error: null } })],
    })

    expect(claude.getByText('A terminal window opened on Air. Finish the sign-in there.')).toBeTruthy()
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
