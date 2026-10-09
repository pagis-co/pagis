// The `coding_session` block (ADR-0033): one card, running or settled.
// It reads the session record and shows the sprite's face, the harness,
// the title, the facts, the state and the mode, the last line of
// activity, where a decision waits, and the end of a settled session.
// "Open" goes to the session page, and Stop closes a session that runs.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, CodingSessionDto } from '../api/client'
import { codingSession } from '../test/appStub'
import { renderInRouter } from '../test/router'
import { formatMoment } from '../timeline'
import { CodingSessionBlock } from './CodingSessionBlock'

const running = {
  ...codingSession,
  last_activity: 'Edited src/login.rs',
  pending: null,
} as CodingSessionDto

const SESSION = '/api/v1/coding-sessions/{coding_session_id}'
const STOP = '/api/v1/coding-sessions/{coding_session_id}/stop'

/** The api of one session. `null` answers that it is not on record,
 *  and `'never'` never answers. */
function stubApi(
  session: CodingSessionDto | null | 'never' = running,
  stop: { error?: unknown; status: number } = { status: 204 },
) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === SESSION) {
        if (session === 'never') return new Promise(() => {})
        return session === null
          ? { error: { error: { code: 'not_found', message: 'no such session' } } }
          : { data: session }
      }
      if (path === '/api/v1/agents') {
        return {
          data: {
            items: [
              {
                id: 'agent-1',
                name: 'Sage',
                job: 'a',
                status: 'active',
                avatar: { sprite: 'pixie', preset: 'mint', colors: {}, accessories: {} },
              },
            ],
          },
        }
      }
      return { data: { items: [] } }
    }),
    POST: vi.fn(async () => ({
      error: stop.error,
      response: new Response(null, { status: stop.status }),
    })),
  }
}

function mount(api: ReturnType<typeof stubApi> = stubApi()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return renderInRouter(
    <QueryClientProvider client={client}>
      <CodingSessionBlock sessionId="session-1" api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

async function block(): Promise<HTMLElement> {
  return screen.findByTestId('coding-session-block')
}

describe('the head and the facts', () => {
  it('shows the harness, the title, the machine, the directory and the branch', async () => {
    mount()

    const card = await block()
    expect(within(card).getByText('Claude Code')).toBeTruthy()
    expect(within(card).getByText('Fix the login bug')).toBeTruthy()
    expect(within(card).getByText('Ada’s laptop')).toBeTruthy()
    const directory = within(card).getByText('/Users/ada/src/app')
    expect(directory.getAttribute('title')).toBe('/Users/ada/src/app')
    expect(within(card).getByText('pagis/fix-login')).toBeTruthy()
    expect(within(card).getByText('Edited src/login.rs')).toBeTruthy()
    expect(within(card).getByText('38% of context')).toBeTruthy()
  })

  it('shows the face of the sprite with no presence ring', async () => {
    mount()

    const card = await block()
    const face = card.querySelector('[data-presence]')
    expect(face?.getAttribute('data-presence')).toBe('none')
  })

  it('names a session in the Computer by the Computer', async () => {
    mount(stubApi({ ...running, place: 'computer', machine_name: null, host_id: null }))

    expect(within(await block()).getByText('Computer')).toBeTruthy()
  })

  it('leaves out a branch that the session does not have', async () => {
    mount(stubApi({ ...running, worktree_branch: null }))

    const card = await block()
    expect(within(card).queryByText('pagis/fix-login')).toBeNull()
  })

  it('leaves out a usage that the harness does not report', async () => {
    mount(
      stubApi({
        ...running,
        usage: { context_used: null, context_size: null, cost_amount: null, cost_currency: null },
      }),
    )

    const card = await block()
    expect(card.textContent).not.toContain('of context')
  })
})

describe('the badges', () => {
  it.each([
    ['starting', 'Opening'],
    ['working', 'Running'],
    ['needs_decision', 'Waits for a decision'],
    ['idle', 'Ready'],
    ['interrupted', 'Interrupted'],
    ['closed', 'Closed'],
    ['failed', 'Failed'],
  ] as const)('shows the %s state as %s', async (state, label) => {
    mount(stubApi({ ...running, state }))

    expect(within(await block()).getByText(label)).toBeTruthy()
  })

  it('shows the mode', async () => {
    mount(stubApi({ ...running, approval_mode: 'agent' }))

    const card = await block()
    expect(await within(card).findByText('Sage approves')).toBeTruthy()
  })

  it('shows the Harness Mode after the approval mode', async () => {
    mount(stubApi({ ...running, harness_mode: 'default', harness_mode_name: 'Manual' }))

    const card = await block()
    const mode = within(card).getByText('Manual')
    expect(mode.classList.contains('ui-badge-neutral')).toBe(true)
    const approval = within(card).getByText('You approve')
    expect(approval.nextElementSibling).toBe(mode)
  })

  it('shows an Unattended Mode in the waiting hue', async () => {
    mount(
      stubApi({
        ...running,
        harness_mode: 'bypassPermissions',
        harness_mode_name: 'Bypass permissions',
        unattended: true,
      }),
    )

    const mode = within(await block()).getByText('Bypass permissions')
    expect(mode.classList.contains('ui-badge-waiting')).toBe(true)
  })

  it('shows no mode badge for a session with no Harness Mode', async () => {
    mount(stubApi({ ...running, harness_mode: null, harness_mode_name: null, unattended: true }))

    const card = await block()
    const approval = within(card).getByText('You approve')
    expect(approval.nextElementSibling?.classList.contains('ui-badge')).not.toBe(true)
  })
})

describe('a decision that waits', () => {
  it('says that a permission waits for the Person in this thread', async () => {
    mount(
      stubApi({
        ...running,
        state: 'needs_decision',
        pending: { kind: 'permission', waits_for: 'person', seq: 4 },
      }),
    )

    const card = await block()
    expect(within(card).getByText('A permission waits for your answer in this thread.')).toBeTruthy()
  })

  it('says that the sprite decides a permission', async () => {
    mount(
      stubApi({
        ...running,
        state: 'needs_decision',
        pending: { kind: 'permission', waits_for: 'agent', seq: 4 },
      }),
    )

    const card = await block()
    expect(await within(card).findByText('Sage decides a permission.')).toBeTruthy()
  })

  it('says that the sprite answers a question of the harness', async () => {
    mount(
      stubApi({
        ...running,
        state: 'needs_decision',
        pending: { kind: 'question', waits_for: 'agent', seq: 4 },
      }),
    )

    const card = await block()
    expect(
      await within(card).findByText('Sage answers a question from Claude Code.'),
    ).toBeTruthy()
  })
})

describe('a settled session', () => {
  const endedAt = Date.parse('2026-09-29T15:06:00Z')

  it('shows its end reason and its end time, and has no Stop', async () => {
    mount(stubApi({ ...running, state: 'closed', end_reason: 'stopped', ended_at: endedAt }))

    const card = await block()
    expect(within(card).getByText('You stopped the session')).toBeTruthy()
    expect(within(card).getByText(formatMoment(endedAt))).toBeTruthy()
    expect(within(card).queryByRole('button', { name: 'Stop' })).toBeNull()
  })

  it('shows an end reason that it does not know as the daemon writes it', async () => {
    mount(stubApi({ ...running, state: 'failed', end_reason: 'quota_gone', ended_at: endedAt }))

    const card = await block()
    expect(within(card).getByText('quota_gone')).toBeTruthy()
    expect(within(card).queryByRole('button', { name: 'Stop' })).toBeNull()
  })
})

describe('before and without the record', () => {
  it('says that it opens the session before the read', async () => {
    mount(stubApi('never'))

    expect(await screen.findByText('Opening the coding session…')).toBeTruthy()
  })

  it('says that a session is not on record', async () => {
    mount(stubApi(null))

    expect(await screen.findByText('That coding session is not on record.')).toBeTruthy()
  })
})

describe('Stop', () => {
  it('posts the stop route once and reads the record again', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(within(await block()).getByRole('button', { name: 'Stop' }))

    await waitFor(() => expect(api.POST).toHaveBeenCalledTimes(1))
    expect(api.POST).toHaveBeenCalledWith(STOP, {
      params: { path: { coding_session_id: 'session-1' } },
    })
    await waitFor(() =>
      expect(api.GET.mock.calls.filter(([path]) => path === SESSION)).toHaveLength(2),
    )
  })

  it('shows an error of the route in an alert', async () => {
    mount(
      stubApi(running, {
        status: 409,
        error: { error: { code: 'conflict', message: 'The coding session is not open.' } },
      }),
    )

    fireEvent.click(within(await block()).getByRole('button', { name: 'Stop' }))

    expect((await screen.findByRole('alert')).textContent).toBe('The coding session is not open.')
  })
})

describe('Open', () => {
  it('goes to the session page', async () => {
    const history = mount()

    fireEvent.click(within(await block()).getByRole('link', { name: 'Open' }))

    expect(await screen.findByTestId('coding-view')).toBeTruthy()
    expect(history.location.pathname).toBe('/coding/session-1')
  })
})

describe('harness text', () => {
  it('shows the last line of activity as text, never as markup', async () => {
    mount(stubApi({ ...running, last_activity: 'Wrote <b>bold</b> and **strong**' }))

    const card = await block()
    expect(within(card).getByText('Wrote <b>bold</b> and **strong**')).toBeTruthy()
    expect(within(card).queryByText('bold')).toBeNull()
    expect(within(card).queryByText('strong')).toBeNull()
  })
})
