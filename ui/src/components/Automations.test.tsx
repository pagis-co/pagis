// The Automations destination (ADR-0022): every Schedule and
// Event Subscription reads with its state, next due, and last Run
// result; each control round-trips to the daemon and the surface shows
// the daemon's answer, the skip-next conflict included; an Agent's
// proposed rule waits in the needs-you queue and becomes a Schedule
// only after the user approves.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { useComposerDraft } from '../state/composerDraft'
import { Automations } from './Automations'

const agent = { id: 'ag1', name: 'Sage', job: 'assistant', status: 'active' }
const channel = { id: 'ch1', title: 'Standup', kind: 'group', agent_ids: ['ag1'], user_member: true }
const dm = { id: 'ch-dm', title: 'Sage', kind: 'dm', agent_ids: ['ag1'], user_member: true }

const schedule = {
  id: 'sc1',
  workspace_id: 'ws1',
  agent_id: 'ag1',
  name: 'Morning briefing',
  instruction: 'Summarize the inbox',
  channel_id: 'ch1',
  root_message_id: null,
  kind: 'cron',
  cron_expression: '0 8 * * *',
  interval_ms: null,
  anchor_at: null,
  timezone: 'America/Los_Angeles',
  scheduled_at: 1_700_000_000_000,
  next_due_at: 1_700_003_600_000,
  last_result: 'completed',
  state: 'active',
  revision: 2,
  approved_revision: 2,
  creator: 'user',
  creating_run_id: null,
  created_at: 1_699_000_000_000,
  updated_at: 1_699_000_000_000,
  archived_at: null,
}

const subscription = {
  id: 'es1',
  workspace_id: 'ws1',
  agent_id: 'ag1',
  connection_id: 'cn1',
  event_kind: 'mail.message_received',
  source_version: '1',
  name: 'Invoice watch',
  instruction: 'File the invoice',
  channel_id: 'ch1',
  root_message_id: null,
  filter: { from: 'billing@example.com' },
  creator: 'user',
  state: 'active',
  revision: 1,
  approved_revision: 1,
  watermark_at: null,
  blocked_reason: null,
  created_at: 1_699_000_000_000,
  updated_at: 1_699_000_000_000,
  archived_at: null,
}

const occurrence = {
  id: 'oc1',
  schedule_id: 'sc1',
  schedule_revision: 2,
  scheduled_at: 1_699_900_000_000,
  processed_at: 1_699_900_000_500,
  outcome: 'woke',
  wakeup_id: 'wk1',
}

const wakeup = {
  id: 'wk1',
  source_kind: 'schedule',
  rule_id: 'sc1',
  rule_revision: 2,
  rule_name: 'Morning briefing',
  agent_id: 'ag1',
  channel_id: 'ch1',
  root_message_id: null,
  instruction: 'Summarize the inbox',
  scheduled_at: 1_699_900_000_000,
  state: 'completed',
  run_id: 'run1',
  source_count: 1,
  created_at: 1_699_900_000_000,
  started_at: 1_699_900_001_000,
}

/** The Request the broker mints when an Agent proposes a Schedule: the
 *  approval happens at the tool call, so no Schedule row exists yet. */
const scheduleRequest = {
  id: 'rq1',
  agent_id: 'ag1',
  run_id: 'run9',
  kind: 'tool_action',
  payload: {
    tool_name: 'schedule_create',
    action_title: 'Create a Schedule',
    body: 'Destination: Standup\nInstruction: Post the digest',
    proposed_rules: [],
  },
  state: 'pending',
  values: null,
  decided_at: null,
  created_at: 1_699_000_000_000,
}

type Row = Record<string, unknown>

type Overrides = {
  schedules?: Row[]
  subscriptions?: Row[]
  requests?: Row[]
  connections?: Row[]
  occurrences?: { items: Row[]; next_cursor?: string | null }[]
  scheduleUpdate?: () => Promise<{ data?: unknown; error?: unknown }>
  subscriptionUpdate?: () => Promise<{ data?: unknown; error?: unknown }>
}

function stubApi(overrides: Overrides = {}) {
  const occurrencePages = overrides.occurrences ?? [
    { items: [occurrence], next_cursor: null },
  ]
  let occurrenceCall = 0
  return {
    GET: vi.fn(async (path: string) => {
      switch (path) {
        case '/api/v1/agents':
          return { data: { items: [agent] } }
        case '/api/v1/channels':
          return { data: { items: [channel, dm] } }
        case '/api/v1/settings/connections':
          return { data: { items: overrides.connections ?? [] } }
        case '/api/v1/requests':
          return { data: { items: overrides.requests ?? [] } }
        // The card reads the Request row itself: the state is the
        // row's, never the queue item's copy.
        case '/api/v1/requests/{request_id}':
          return { data: scheduleRequest }
        case '/api/v1/schedules':
          return { data: { items: overrides.schedules ?? [schedule] } }
        case '/api/v1/schedules/{schedule_id}':
          return { data: (overrides.schedules ?? [schedule])[0] }
        case '/api/v1/schedules/{schedule_id}/occurrences': {
          const page =
            occurrencePages[Math.min(occurrenceCall, occurrencePages.length - 1)]
          occurrenceCall += 1
          return { data: page }
        }
        case '/api/v1/schedules/{schedule_id}/wakeups':
          return { data: { items: [wakeup], next_cursor: null } }
        case '/api/v1/event-subscriptions':
          return { data: { items: overrides.subscriptions ?? [subscription] } }
        case '/api/v1/event-subscriptions/{subscription_id}':
          return {
            data: {
              ...(overrides.subscriptions ?? [subscription])[0],
              last_collection: {
                id: 'sb1',
                collected_at: 1_699_900_000_000,
                collected_count: 3,
                stored_count: 1,
                wakeup_count: 1,
                outcome: 'succeeded',
                detail: null,
              },
              last_successful_collection: null,
              last_matched_event: null,
            },
          }
        case '/api/v1/event-subscriptions/{subscription_id}/events':
          return { data: { items: [], next_cursor: null } }
        case '/api/v1/event-subscriptions/{subscription_id}/wakeups':
          return { data: { items: [], next_cursor: null } }
        default:
          throw new Error(`unexpected GET ${path}`)
      }
    }),
    POST: vi.fn(async (path: string) => {
      if (path === '/api/v1/schedules/{schedule_id}') {
        return overrides.scheduleUpdate
          ? overrides.scheduleUpdate()
          : { data: { ...schedule, state: 'paused' } }
      }
      if (path === '/api/v1/event-subscriptions/{subscription_id}') {
        return overrides.subscriptionUpdate
          ? overrides.subscriptionUpdate()
          : { data: { ...subscription, state: 'paused' } }
      }
      if (path === '/api/v1/requests/{request_id}/decision') {
        return { data: { ...scheduleRequest, state: 'approved' } }
      }
      throw new Error(`unexpected POST ${path}`)
    }),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  const onOpenChannel = vi.fn()
  render(
    <QueryClientProvider client={queryClient}>
      <Automations
        api={api as unknown as ApiClient}
        onClose={vi.fn()}
        onOpenNav={vi.fn()}
        onOpenChannel={onOpenChannel}
      />
    </QueryClientProvider>,
  )
  return { onOpenChannel }
}

describe('Automations', () => {
  it('lists every rule with its state, next due, and last Run result', async () => {
    mount(stubApi())

    expect(await screen.findByText('Morning briefing')).toBeTruthy()
    expect(
      screen.getByText(`Next ${new Date(schedule.next_due_at).toLocaleString()}`),
    ).toBeTruthy()
    expect(screen.getByText('Last completed')).toBeTruthy()

    expect(await screen.findByText('Invoice watch')).toBeTruthy()
    expect(screen.getByText('mail.message_received')).toBeTruthy()
  })

  it('the detail body carries the revision, creator, Agent, and history', async () => {
    mount(stubApi())

    fireEvent.click(await screen.findByText('Morning briefing'))

    expect(await screen.findByText('0 8 * * * in America/Los_Angeles')).toBeTruthy()
    expect(screen.getByText('Sage')).toBeTruthy()
    expect(screen.getByText('Standup')).toBeTruthy()
    expect(screen.getByText('2')).toBeTruthy()
    expect(screen.getByText('user')).toBeTruthy()
    expect(await screen.findByText(/woke \(revision 2\)/)).toBeTruthy()
    expect(await screen.findByText(/completed \(1 source\)/)).toBeTruthy()
  })

  it('pause round-trips to the daemon', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(await screen.findByText('Morning briefing'))
    fireEvent.click(await screen.findByText('Pause'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/schedules/{schedule_id}', {
        params: { path: { schedule_id: 'sc1' } },
        body: { action: 'pause' },
      }),
    )
  })

  it('a paused Schedule offers Resume instead of Pause', async () => {
    const api = stubApi({ schedules: [{ ...schedule, state: 'paused' }] })
    mount(api)

    fireEvent.click(await screen.findByText('Morning briefing'))
    fireEvent.click(await screen.findByText('Resume'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/schedules/{schedule_id}', {
        params: { path: { schedule_id: 'sc1' } },
        body: { action: 'resume' },
      }),
    )
  })

  it('skip-next sends the due instant the surface saw', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(await screen.findByText('Morning briefing'))
    fireEvent.click(await screen.findByText('Skip next'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/schedules/{schedule_id}', {
        params: { path: { schedule_id: 'sc1' } },
        body: { action: 'skip_next', expected_due_at: schedule.next_due_at },
      }),
    )
  })

  it('a lost skip-next race shows the daemon words, not silence', async () => {
    const api = stubApi({
      scheduleUpdate: async () => ({
        error: {
          error: {
            code: 'conflict',
            message: 'the Schedule became due before skip-next completed',
          },
        },
      }),
    })
    mount(api)

    fireEvent.click(await screen.findByText('Morning briefing'))
    fireEvent.click(await screen.findByText('Skip next'))

    expect(
      await screen.findByText('the Schedule became due before skip-next completed'),
    ).toBeTruthy()
  })

  it('archive round-trips to the daemon', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(await screen.findByText('Morning briefing'))
    fireEvent.click(await screen.findByText('Archive'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/schedules/{schedule_id}', {
        params: { path: { schedule_id: 'sc1' } },
        body: { action: 'archive' },
      }),
    )
  })

  it('edit sends one revision with the expected revision behind it', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(await screen.findByText('Morning briefing'))
    fireEvent.click(await screen.findByText('Edit'))
    fireEvent.change(screen.getByLabelText('Name'), {
      target: { value: 'Evening briefing' },
    })
    fireEvent.click(screen.getByText('Save'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/schedules/{schedule_id}', {
        params: { path: { schedule_id: 'sc1' } },
        body: {
          action: 'edit',
          expected_revision: 2,
          name: 'Evening briefing',
          instruction: 'Summarize the inbox',
        },
      }),
    )
  })

  it('the occurrence history pages on the daemon cursor', async () => {
    const api = stubApi({
      occurrences: [
        { items: [occurrence], next_cursor: 'oc1' },
        {
          items: [{ ...occurrence, id: 'oc0', outcome: 'skipped' }],
          next_cursor: null,
        },
      ],
    })
    mount(api)

    fireEvent.click(await screen.findByText('Morning briefing'))
    fireEvent.click(await screen.findByText('Show more'))

    expect(await screen.findByText(/skipped \(revision 2\)/)).toBeTruthy()
    await waitFor(() =>
      expect(api.GET).toHaveBeenCalledWith(
        '/api/v1/schedules/{schedule_id}/occurrences',
        {
          params: {
            path: { schedule_id: 'sc1' },
            query: { before: 'oc1', limit: 20 },
          },
        },
      ),
    )
  })

  it('an Event Subscription pauses and edits through its own endpoint', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(await screen.findByText('Invoice watch'))
    fireEvent.click(await screen.findByText('Pause'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/event-subscriptions/{subscription_id}',
        {
          params: { path: { subscription_id: 'es1' } },
          body: { action: 'pause' },
        },
      ),
    )
  })

  it('an Event Subscription has no skip-next', async () => {
    mount(stubApi())

    fireEvent.click(await screen.findByText('Invoice watch'))
    await screen.findByText('Pause')

    expect(screen.queryByText('Skip next')).toBeNull()
  })

  it('a rule an Agent proposed waits in the needs-you queue, not the list', async () => {
    const api = stubApi({ requests: [scheduleRequest], schedules: [] })
    mount(api)

    expect(await screen.findByText('Schedule waits for you')).toBeTruthy()
    expect(screen.getByText('Create a Schedule')).toBeTruthy()
    expect(screen.getByText('No Schedule yet.')).toBeTruthy()

    fireEvent.click(screen.getByText('Approve'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/requests/{request_id}/decision',
        {
          params: { path: { request_id: 'rq1' } },
          body: { decision: 'approved', scope: undefined, values: undefined },
        },
      ),
    )
  })

  it('a blocked collector and a Connection to repair both reach the queue', async () => {
    mount(
      stubApi({
        connections: [
          {
            id: 'cn1',
            provider: 'google',
            alias: 'work',
            display_name: 'Work Gmail',
            account: 'a@example.com',
            status: 'reauth_required',
            created_at: 1,
          },
        ],
        subscriptions: [{ ...subscription, blocked_reason: 'grant_missing' }],
      }),
    )

    expect(await screen.findByText('Collector is blocked')).toBeTruthy()
    expect(screen.getByText('Invoice watch: grant_missing')).toBeTruthy()
    expect(screen.getByText('Connection needs you')).toBeTruthy()
  })

  it('an empty workspace says so instead of showing bare headings', async () => {
    mount(stubApi({ schedules: [], subscriptions: [] }))

    expect(await screen.findByText('No Schedule yet.')).toBeTruthy()
    expect(screen.getByText('No Event Subscription yet.')).toBeTruthy()
    expect(screen.getByText('Nothing waits for you.')).toBeTruthy()
  })

  it('every section says in one line what it holds', async () => {
    mount(stubApi())

    expect(
      await screen.findByText(
        'Rules and connections that wait for your decision.',
      ),
    ).toBeTruthy()
    expect(
      screen.getByText(
        'Work a sprite repeats on a clock, such as every weekday morning.',
      ),
    ).toBeTruthy()
    expect(
      screen.getByText(
        'Work a sprite starts when something happens, such as new mail from your accountant.',
      ),
    ).toBeTruthy()
  })

  it('every rule shows the face of the agent that owns it', async () => {
    mount(stubApi())

    await screen.findByText('Morning briefing')
    const faces = document.querySelectorAll('.automations-row .ui-avatar')
    expect(faces.length).toBe(2)
    expect([...faces].every((face) => face.querySelector('[role=img]')?.getAttribute('aria-label') === 'Sage, Pixie avatar')).toBe(true)
  })

  it('an empty section asks an agent to set one up, in that agent DM', async () => {
    useComposerDraft.setState({ byScope: {} })
    const { onOpenChannel } = mount(stubApi({ schedules: [], subscriptions: [] }))

    const asks = await screen.findAllByRole('button', {
      name: 'Ask a sprite to set one up',
    })
    expect(asks.length).toBe(3)
    fireEvent.pointerDown(
      asks[1],
      new PointerEvent('pointerdown', { bubbles: true, ctrlKey: false, button: 0 }),
    )
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Sage' }))

    await waitFor(() => expect(onOpenChannel).toHaveBeenCalledWith('ch-dm'))
    expect(useComposerDraft.getState().byScope['ch-dm']).toBe(
      'Please set up a schedule for me. Run it every weekday at 08:00 and do:',
    )
  })

  it('the history inside a schedule says in one line what it holds', async () => {
    mount(stubApi())

    fireEvent.click(await screen.findByText('Morning briefing'))

    expect(
      await screen.findByText('Every time this schedule was due, and what came of it.'),
    ).toBeTruthy()
    expect(
      screen.getByText('Every time a sprite woke up to do this work.'),
    ).toBeTruthy()
  })
})
