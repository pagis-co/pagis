// Home, the Report of the Chief of Staff (ADR-0022): the date
// and the brief line head the page, the queue carries the decision, a
// frame that puts a run in front of the reader lands in the queue with
// no reload, the Report prose reads under it, and the composer speaks
// to the Chief of Staff.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest'

import type { ApiClient, CallSummaryDto, EventRow, RunDto } from '../../api/client'
import { useComposerDraft } from '../../state/composerDraft'
import { usePresence } from '../../state/presence'
import { formatClock } from '../../timeline'
import { Home } from './Home'

const NOW = Date.now()

function run(fields: Partial<RunDto>): RunDto {
  return {
    id: 'run-1',
    agent_id: 'agent-1',
    channel_id: 'channel-1',
    root_message_id: null,
    trigger_kind: 'message',
    trigger_ref: null,
    hop_count: 0,
    state: 'completed',
    error: null,
    started_at: NOW,
    ended_at: NOW,
    created_at: NOW,
    duration_ms: 1200,
    ...fields,
  }
}

const pendingRequest = {
  id: 'request-1',
  agent_id: 'agent-1',
  run_id: 'run-1',
  kind: 'tool_action',
  state: 'pending',
  payload: { action_title: 'Open a file', body: 'host__read' },
  created_at: NOW,
  decided_at: null,
}

function missedCall(fields: Partial<CallSummaryDto> = {}): CallSummaryDto {
  return {
    id: 'call-1',
    agent_id: 'agent-1',
    agent_name: 'Sage',
    own_e164: '+14155550123',
    direction: 'inbound',
    remote_e164: '+14155550199',
    purpose: '',
    tier: 'unknown',
    state: 'ended',
    outcome: 'no_answer',
    ended_reason: 'no_answer',
    classification: null,
    message_left: false,
    recording_artifact_id: null,
    created_at: NOW,
    answered_at: null,
    ended_at: NOW,
    ...fields,
  }
}

const reportMessage = {
  id: 'msg-report',
  channel_id: 'channel-1',
  parent_message_id: 'msg-root',
  author_kind: 'agent',
  author_agent_id: 'agent-1',
  run_id: 'run-report',
  status: 'complete',
  blocks: [{ type: 'text', text: 'Nothing else ran today.' }],
  text_content: 'Nothing else ran today.',
  pending_id: null,
  created_at: NOW,
  completed_at: NOW,
}

function stubApi(
  options: {
    requests?: unknown[]
    calls?: CallSummaryDto[]
    failRequests?: boolean
    chief?: string | null
    report?: unknown
    writingRunId?: string | null
    keypad?: { failed_attempts: number; suspended_until: number | null }
    failedRuns?: RunDto[]
  } = {},
) {
  const post = vi.fn(async () => ({ data: { ok: true } }))
  const remove = vi.fn(async () => ({ error: undefined, response: { ok: true } }))
  const api = {
    GET: vi.fn(async (path: string, init?: { params?: { query?: { state?: string } } }) => {
      if (path === '/api/v1/agents') {
        return {
          data: {
            items: [
              { id: 'agent-1', name: 'Sage', job: 'assistant', status: 'active' },
              { id: 'agent-2', name: 'Nova', job: 'assistant', status: 'active' },
            ],
          },
        }
      }
      if (path === '/api/v1/channels') {
        return {
          data: {
            items: [
              { id: 'channel-1', title: 'Sage', kind: 'dm', agent_ids: ['agent-1'], user_member: true },
            ],
          },
        }
      }
      if (path === '/api/v1/workspace') {
        return {
          data: {
            id: 'ws-1',
            name: 'Workspace',
            timezone: 'UTC',
            chief_of_staff_agent_id:
              options.chief === undefined ? 'agent-1' : options.chief,
            report_schedule_id: 'sch-report',
          },
        }
      }
      if (path === '/api/v1/workspace/report') {
        return {
          data: {
            schedule_id: 'sch-report',
            next_due_at: NOW + 86_400_000,
            message: options.report === undefined ? reportMessage : options.report,
            writing_run_id: options.writingRunId ?? null,
          },
        }
      }
      if (path === '/api/v1/calls') {
        return { data: { items: options.calls ?? [] } }
      }
      if (path === '/api/v1/settings/trust-list') {
        return {
          data: {
            items: [],
            own_addresses: [],
            keypad_code: {
              configured: true,
              failed_attempts: 0,
              suspended_until: null,
              ...options.keypad,
            },
          },
        }
      }
      if (path === '/api/v1/requests') {
        if (options.failRequests) throw new Error('Unavailable')
        return { data: { items: options.requests ?? [] } }
      }
      if (path === '/api/v1/requests/{request_id}') {
        return { data: pendingRequest }
      }
      if (path === '/api/v1/runs') {
        const state = init?.params?.query?.state
        if (state === 'completed') return { data: { items: [run({ id: 'done-1' })] } }
        if (state === 'failed') return { data: { items: options.failedRuns ?? [] } }
        return { data: { items: [] } }
      }
      return { data: { items: [] } }
    }),
    POST: post,
    DELETE: remove,
  }
  return { api: api as unknown as ApiClient, post, remove }
}

function mount(api: ApiClient) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  const opened = { channel: [] as string[], run: [] as string[] }
  render(
    <QueryClientProvider client={queryClient}>
      <Home
        api={api}
        onOpenChannel={(id) => opened.channel.push(id)}
        onOpenRun={(id) => opened.run.push(id)}
        onOpenNav={() => undefined}
      />
    </QueryClientProvider>,
  )
  return opened
}

/** One firehose frame, folded the way the shell folds it. */
function frame(type: string, event: Partial<EventRow>) {
  act(() =>
    usePresence.getState().applyFrame(type, {
      id: 'event-1',
      created_at: NOW,
      event_type: type,
      payload: {},
      ...event,
    } as EventRow),
  )
}

beforeEach(() => {
  useComposerDraft.setState({ byScope: {} })
  usePresence.setState({
    runs: {},
    onCall: {},
    unread: {},
    selectedChannelId: null,
    seeded: false,
  })
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response('missing', { status: 404 })),
  )
})

it('does not report an empty queue when approvals fail to load', async () => {
  const options = { failRequests: true }
  const { api } = stubApi(options)
  mount(api)
  expect(await screen.findByText('Could not load your decisions')).toBeTruthy()
  expect(screen.queryByText('Nothing needs you.')).toBeNull()
  options.failRequests = false
  fireEvent.click(within(screen.getByRole('region', { name: 'Needs you' })).getByRole('button', { name: 'Try again' }))
  expect(await screen.findByText('Nothing needs you.')).toBeTruthy()
})

describe('Needs you', () => {
  it('says plainly when nothing needs the reader', async () => {
    const { api } = stubApi()
    mount(api)

    expect(await screen.findByText('Nothing needs you.')).toBeTruthy()
  })

  it('renders a pending approval with a working Approve', async () => {
    const { api, post } = stubApi({ requests: [pendingRequest] })
    mount(api)

    expect(await screen.findByText('Sage needs your approval')).toBeTruthy()
    fireEvent.click(await screen.findByRole('button', { name: 'Approve' }))

    await waitFor(() =>
      expect(post).toHaveBeenCalledWith('/api/v1/requests/{request_id}/decision', {
        params: { path: { request_id: 'request-1' } },
        body: { decision: 'approved', scope: undefined, values: undefined },
      }),
    )
  })

  it('renders a pending approval with a working Deny', async () => {
    const { api, post } = stubApi({ requests: [pendingRequest] })
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Deny' }))

    await waitFor(() =>
      expect(post).toHaveBeenCalledWith('/api/v1/requests/{request_id}/decision', {
        params: { path: { request_id: 'request-1' } },
        body: { decision: 'denied', scope: undefined, values: undefined },
      }),
    )
  })

  it('adds a queue item from a run that waits for the user, with no reload', async () => {
    const { api } = stubApi()
    const opened = mount(api)
    expect(await screen.findByText('Nothing needs you.')).toBeTruthy()

    frame('run.state_changed', {
      run_id: 'run-7',
      agent_id: 'agent-1',
      channel_id: 'channel-1',
      payload: { to: 'waiting_for_user', trigger_kind: 'message' },
    })

    expect(await screen.findByText('Sage waits for your answer')).toBeTruthy()
    expect(screen.queryByText('Nothing needs you.')).toBeNull()

    fireEvent.click(screen.getByRole('button', { name: 'Open conversation' }))
    expect(opened.channel).toEqual(['channel-1'])
  })

  it('draws a queue row in the one card frame, with one action in the footer', async () => {
    const { api } = stubApi()
    mount(api)
    expect(await screen.findByText('Nothing needs you.')).toBeTruthy()

    frame('run.state_changed', {
      run_id: 'run-7',
      agent_id: 'agent-1',
      channel_id: 'channel-1',
      payload: { to: 'waiting_for_user', trigger_kind: 'message' },
    })

    const line = await screen.findByText('Sage waits for your answer')
    const card = line.closest('.ui-frame')!
    expect(card.className).toContain('block-card')
    expect(card.className).toContain('home-queue-card')
    expect(card.getAttribute('data-kind')).toBe('waiting')
    expect(card.querySelector('.home-queue-dot')).not.toBeNull()
    const footer = card.querySelector('.block-card-footer')!
    expect(footer.querySelectorAll('button')).toHaveLength(1)
    expect(footer.textContent).toContain('Open conversation')
  })

  it('lands a missed call on the agent DM with the call back message written', async () => {
    const { api } = stubApi({ calls: [missedCall()] })
    const opened = mount(api)

    expect(await screen.findByText('Sage missed a call from +14155550199')).toBeTruthy()
    expect(screen.getByText('Nobody answered.')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Call back' }))

    expect(opened.channel).toEqual(['channel-1'])
    expect(useComposerDraft.getState().byScope['channel-1']).toBe(
      'Please call +14155550199 back. They called and nobody answered.',
    )
  })

  it('dismisses a failure when the reader opens its run', async () => {
    const { api, post } = stubApi({
      failedRuns: [run({ id: 'run-2', state: 'failed', error: 'the request timed out' })],
    })
    const opened = mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Open run' }))

    expect(opened.run).toEqual(['run-2'])
    await waitFor(() =>
      expect(post).toHaveBeenCalledWith('/api/v1/runs/{run_id}/dismiss', {
        params: { path: { run_id: 'run-2' } },
      }),
    )
  })

  it('dismisses a failure with no visit to its run', async () => {
    const { api, post } = stubApi({
      failedRuns: [run({ id: 'run-2', state: 'failed', error: 'the request timed out' })],
    })
    const opened = mount(api)

    const line = await screen.findByText('Sage could not finish the work')
    const card = line.closest('.ui-frame') as HTMLElement
    fireEvent.click(within(card).getByRole('button', { name: 'Dismiss' }))

    expect(opened.run).toEqual([])
    await waitFor(() =>
      expect(post).toHaveBeenCalledWith('/api/v1/runs/{run_id}/dismiss', {
        params: { path: { run_id: 'run-2' } },
      }),
    )
  })

  it('dismisses a missed call when the reader calls back, or dismisses it alone', async () => {
    const { api, post } = stubApi({
      calls: [missedCall(), missedCall({ id: 'call-2', remote_e164: '+14155550188' })],
    })
    mount(api)

    fireEvent.click((await screen.findAllByRole('button', { name: 'Call back' }))[0])
    const second = screen.getByText('Sage missed a call from +14155550188').closest('.ui-frame')
    fireEvent.click(within(second as HTMLElement).getByRole('button', { name: 'Dismiss' }))

    await waitFor(() => {
      expect(post).toHaveBeenCalledWith('/api/v1/calls/{call_id}/dismiss', {
        params: { path: { call_id: 'call-1' } },
      })
      expect(post).toHaveBeenCalledWith('/api/v1/calls/{call_id}/dismiss', {
        params: { path: { call_id: 'call-2' } },
      })
    })
  })

  it('tells the reader when a keypad delay starts, and clears the count', async () => {
    // Noon, so the end of the delay falls on the same day and reads as a
    // clock time with no date.
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(new Date(2026, 0, 15, 12, 0))
    onTestFinished(() => { vi.useRealTimers() })
    const until = Date.now() + 60_000
    const { api, remove } = stubApi({
      keypad: { failed_attempts: 6, suspended_until: until },
    })
    mount(api)

    const line = await screen.findByText('Callers entered a wrong keypad code 6 times')
    const card = line.closest('.ui-frame')!
    expect(card.getAttribute('data-kind')).toBe('keypad')
    expect(card.textContent).toContain(`until ${formatClock(until)}`)

    fireEvent.click(within(card as HTMLElement).getByRole('button', { name: 'Clear the count' }))

    await waitFor(() =>
      expect(remove).toHaveBeenCalledWith('/api/v1/settings/keypad-code/failures'),
    )
  })

  it('leaves an answered inbound call out of the queue', async () => {
    const { api } = stubApi({ calls: [missedCall({ outcome: 'answered' })] })
    mount(api)

    expect(await screen.findByText('Nothing needs you.')).toBeTruthy()
  })
})

describe('the brief', () => {
  it('heads the page with the day and whose brief this is', async () => {
    const { api } = stubApi()
    mount(api)

    expect(
      await screen.findByRole('heading', { level: 2, name: /,/ }),
    ).toBeTruthy()
    expect(await screen.findByText(/^Sage’s brief · /)).toBeTruthy()
  })

  it('reads the Report the Chief of Staff wrote', async () => {
    const { api } = stubApi()
    mount(api)

    const brief = await screen.findByTestId('home-report')
    expect(within(brief).getByText('Nothing else ran today.')).toBeTruthy()
  })

  it('says so before the first Report is written', async () => {
    const { api } = stubApi({ report: null })
    mount(api)

    expect(await screen.findByText('No brief yet')).toBeTruthy()
    expect(await screen.findByText('Sage has written no brief yet')).toBeTruthy()
  })

  it('asks the Report Schedule to run when the reader wants one now', async () => {
    const { api, post } = stubApi({ report: null })
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Write a report now' }))

    await waitFor(() =>
      expect(post).toHaveBeenCalledWith('/api/v1/schedules/{schedule_id}/run', {
        params: { path: { schedule_id: 'sch-report' } },
      }),
    )
  })

  it('says a Report is on its way while the Run writes it', async () => {
    const { api } = stubApi({ writingRunId: 'run-report' })
    mount(api)
    expect(await screen.findByTestId('home-report')).toBeTruthy()

    frame('run.state_changed', {
      run_id: 'run-report',
      agent_id: 'agent-1',
      channel_id: 'channel-1',
      payload: { to: 'running', trigger_kind: 'schedule' },
    })

    expect(await screen.findByText('Sage is writing your brief')).toBeTruthy()
    expect(
      (await screen.findByRole('button', { name: 'Write a report now' })).hasAttribute(
        'disabled',
      ),
    ).toBe(true)
  })

  it('speaks of the office when no Agent is the Chief of Staff', async () => {
    const { api } = stubApi({ chief: null, report: null })
    mount(api)

    expect(await screen.findByText('No sprite is the chief of staff')).toBeTruthy()
  })
})

describe('the work record', () => {
  it('lists the work that finished, today and before', async () => {
    const { api } = stubApi()
    const opened = mount(api)

    const record = await screen.findByRole('region', { name: 'Work record' })
    expect(within(record).getByText('Today and before')).toBeTruthy()
    fireEvent.click(await within(record).findByRole('button', { name: /Sage/ }))
    expect(opened.run).toEqual(['done-1'])
  })
})

describe('the composer', () => {
  it('addresses the Chief of Staff', async () => {
    const { api } = stubApi()
    mount(api)

    expect(await screen.findByPlaceholderText('Message Sage')).toBeTruthy()
  })

  it('writes no composer when no Agent is the Chief of Staff', async () => {
    const { api } = stubApi({ chief: null, report: null })
    mount(api)

    await screen.findByText('No sprite is the chief of staff')
    expect(screen.queryByPlaceholderText(/^Message /)).toBeNull()
  })
})
