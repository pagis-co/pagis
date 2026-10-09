// The Coding sessions section of the Access tab (ADR-0033): the Person
// sets the widest Session Approval Mode of a sprite on each computer of
// theirs that can start a Coding Harness, and whether the sprite may use
// Unattended Modes there. The host Grant of the sprite on that computer
// holds both, and no Grant reads "Ask me" with no Unattended Modes.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { AgentDto, ApiClient } from '../../api/client'
import { CodingSessionAccess } from './CodingSessionAccess'

const sage = { id: 'ag1', name: 'Sage' } as AgentDto

function host(overrides: Record<string, unknown> = {}) {
  return {
    id: 'h-1',
    name: 'Air',
    platform: 'macos',
    capabilities: ['shell', 'harness:claude-code'],
    present: true,
    last_seen_at: 1_700_000_000_000,
    ...overrides,
  }
}

function grant(overrides: Record<string, unknown> = {}) {
  return {
    id: 'g-1',
    agent_id: 'ag1',
    agent_name: 'Sage',
    allow: [],
    capabilities: [],
    created_at: 1_700_000_000_000,
    resource_kind: 'host',
    resource_id: 'h-1',
    revision: 1,
    session_approval_mode: 'agent',
    ...overrides,
  }
}

/** An API whose reads answer `hosts` and `grants`, and whose changes
 *  answer `put`. Each read of the Grants answers the rows of `grants`
 *  at that time. */
function stubApi(
  hosts: unknown[],
  grants: unknown[] = [],
  put: () => Promise<unknown> = async () => ({ data: grant() }),
) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/hosts') return { data: { items: hosts } }
      if (path === '/api/v1/grants') return { data: { items: [...grants] } }
      return { data: { items: [] } }
    }),
    PUT: vi.fn(put),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={queryClient}>
      <CodingSessionAccess api={api as unknown as ApiClient} agent={sage} />
    </QueryClientProvider>,
  )
}

const combobox = (machine: string) =>
  screen.findByRole('combobox', { name: `Approvals for coding sessions on ${machine}` })

const UNATTENDED = 'Allow modes that act without asking'
const WARNING = 'A coding harness can then run any command as you on Air, with no question.'
const unattendedSwitch = () => screen.findByRole('switch', { name: UNATTENDED })

describe('CodingSessionAccess', () => {
  it('shows a row for each computer that can start a coding harness, and none for another', async () => {
    mount(
      stubApi([
        host(),
        host({ id: 'h-2', name: 'Studio', capabilities: ['harness:codex'] }),
        host({ id: 'h-3', name: 'Phone', platform: 'ios', capabilities: [] }),
        host({ id: 'h-4', name: 'Server', capabilities: ['shell'] }),
      ]),
    )

    expect(await combobox('Air')).toBeTruthy()
    expect(await combobox('Studio')).toBeTruthy()
    expect(screen.queryByRole('combobox', { name: /Phone/ })).toBeNull()
    expect(screen.queryByRole('combobox', { name: /Server/ })).toBeNull()
    expect(
      screen.getByText(
        'The widest approval mode Sage may use for a coding session on each computer.',
      ),
    ).toBeTruthy()
  })

  it('reads the mode from the host Grant of the sprite on the computer', async () => {
    mount(
      stubApi(
        [host()],
        [
          grant(),
          grant({ id: 'g-2', agent_id: 'ag2', agent_name: 'Rex', session_approval_mode: 'person' }),
        ],
      ),
    )

    await waitFor(async () =>
      expect((await combobox('Air')).textContent).toContain('Let the sprite decide'),
    )
    expect(
      screen.getByText(
        'Sage answers each permission that Pagis does not allow by its own rules, or asks you.',
      ),
    ).toBeTruthy()
  })

  it('reads "Ask me" on a computer where the sprite has no host Grant', async () => {
    mount(stubApi([host()], [grant({ resource_id: 'h-9' })]))

    await waitFor(async () => expect((await combobox('Air')).textContent).toContain('Ask me'))
    expect(
      screen.getByText('You answer each permission that Pagis does not allow by its own rules.'),
    ).toBeTruthy()
  })

  it('calls the route once with the sprite, the computer and the mode', async () => {
    const user = userEvent.setup()
    const api = stubApi([host()])
    mount(api)

    await user.click(await combobox('Air'))
    await user.click(await screen.findByRole('option', { name: 'Let the sprite decide' }))

    await waitFor(() => expect(api.PUT).toHaveBeenCalledTimes(1))
    expect(api.PUT).toHaveBeenCalledWith(
      '/api/v1/agents/{agent_id}/hosts/{host_id}/session-approval-mode',
      { params: { path: { agent_id: 'ag1', host_id: 'h-1' } }, body: { mode: 'agent' } },
    )
  })

  it('offers the modes "Ask me" and "Let the sprite decide" alone', async () => {
    const user = userEvent.setup()
    mount(stubApi([host()]))

    await user.click(await combobox('Air'))

    const options = await screen.findAllByRole('option')
    expect(options.map((option) => option.textContent)).toEqual([
      'Ask me',
      'Let the sprite decide',
    ])
  })

  it('shows an alert and the saved mode again when the change fails', async () => {
    const user = userEvent.setup()
    const api = stubApi([host()], [], async () => ({
      error: { error: { code: 'forbidden', message: 'Sage cannot use this computer.' } },
    }))
    mount(api)

    await user.click(await combobox('Air'))
    await user.click(await screen.findByRole('option', { name: 'Let the sprite decide' }))

    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toBe('Sage cannot use this computer.')
    const row = screen.getByTestId('coding-session-access-row')
    await waitFor(() =>
      expect(within(row).getByRole('combobox').textContent).toContain('Ask me'),
    )
  })

  it('says how to add a computer when none can start a coding harness', async () => {
    mount(stubApi([host({ capabilities: ['shell'] })]))

    expect(
      await screen.findByText(
        'No computer of yours can start a coding harness. Open the Pagis client on a computer that has one.',
      ),
    ).toBeTruthy()
    expect(screen.queryByRole('combobox')).toBeNull()
  })

  describe('the allowance of Unattended Modes', () => {
    it('shows the saved allowance of the host Grant', async () => {
      mount(stubApi([host()], [grant({ unattended_modes: true })]))

      await waitFor(async () =>
        expect((await unattendedSwitch()).getAttribute('aria-checked')).toBe('true'),
      )
      expect(screen.getByText(WARNING)).toBeTruthy()
      expect(
        screen.getByText(
          'Sage may run a coding harness in a mode that does not ask first, such as Bypass ' +
            'permissions of Claude Code, and a harness that never asks, such as pi.',
        ),
      ).toBeTruthy()
    })

    it('is off on a computer where the sprite has no host Grant', async () => {
      mount(stubApi([host()], [grant({ resource_id: 'h-9', unattended_modes: true })]))

      expect((await unattendedSwitch()).getAttribute('aria-checked')).toBe('false')
      expect(screen.queryByText(WARNING)).toBeNull()
    })

    it('sends the allowance once and shows the warning', async () => {
      const user = userEvent.setup()
      const grants = [grant({ unattended_modes: false })]
      const api = stubApi([host()], grants, async () => {
        grants[0] = grant({ unattended_modes: true, revision: 2 })
        return { data: grants[0] }
      })
      mount(api)

      await user.click(await unattendedSwitch())

      await waitFor(() => expect(api.PUT).toHaveBeenCalledTimes(1))
      expect(api.PUT).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/hosts/{host_id}/unattended-modes',
        { params: { path: { agent_id: 'ag1', host_id: 'h-1' } }, body: { allowed: true } },
      )
      expect(await screen.findByText(WARNING)).toBeTruthy()
      await waitFor(async () =>
        expect((await unattendedSwitch()).getAttribute('aria-checked')).toBe('true'),
      )
    })

    it('shows an alert and the saved allowance again when the change fails', async () => {
      const user = userEvent.setup()
      const api = stubApi([host()], [grant({ unattended_modes: false })], async () => ({
        error: { error: { code: 'forbidden', message: 'Sage cannot use this computer.' } },
      }))
      mount(api)

      await user.click(await unattendedSwitch())

      const alert = await screen.findByRole('alert')
      expect(alert.textContent).toBe('Sage cannot use this computer.')
      await waitFor(async () =>
        expect((await unattendedSwitch()).getAttribute('aria-checked')).toBe('false'),
      )
      expect(screen.queryByText(WARNING)).toBeNull()
    })
  })
})
