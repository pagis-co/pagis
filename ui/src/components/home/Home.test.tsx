// Home, the Report of the Chief of Staff (ADR-0022): the date
// and the brief line head the page, the daemon's Needs-You Queue
// carries the decisions with an inline action on each row, the Report
// prose reads under it, and the composer speaks to the Chief of Staff.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest'

import type { ApiClient, EventRow, NeedsYouItem, RunDto } from '../../api/client'
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
    title: 'Book the Austin trip',
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

type Item<Kind extends NeedsYouItem['kind']> = Extract<NeedsYouItem, { kind: Kind }>

function approval(fields: Partial<Item<'approval'>> = {}): Item<'approval'> {
  return {
    kind: 'approval',
    id: 'request:request-1',
    agent_id: 'agent-1',
    line: 'Sage needs your approval',
    url: '/c/channel-1',
    at: NOW,
    request_id: 'request-1',
    request_kind: 'tool_action',
    title: 'Open a file',
    body: 'host__read',
    ...fields,
  }
}

function waiting(fields: Partial<Item<'waiting'>> = {}): Item<'waiting'> {
  return {
    kind: 'waiting',
    id: 'run:run-7',
    agent_id: 'agent-1',
    line: 'Sage waits for your answer',
    url: '/c/channel-1',
    at: NOW,
    run_id: 'run-7',
    channel_id: 'channel-1',
    ...fields,
  }
}

function keypad(fields: Partial<Item<'keypad'>> = {}): Item<'keypad'> {
  return {
    kind: 'keypad',
    id: 'keypad',
    line: 'Callers entered a wrong keypad code 6 times',
    url: '/',
    at: NOW + 60_000,
    failed_attempts: 6,
    suspended_until: NOW + 60_000,
    ...fields,
  }
}

function missedCall(fields: Partial<Item<'call'>> = {}): Item<'call'> {
  return {
    kind: 'call',
    id: 'call:call-1',
    agent_id: 'agent-1',
    line: 'Sage missed a call from +14155550199',
    url: '/',
    at: NOW,
    call_id: 'call-1',
    remote_e164: '+14155550199',
    left_message: false,
    ...fields,
  }
}

function failure(fields: Partial<Item<'failed'>> = {}): Item<'failed'> {
  return {
    kind: 'failed',
    id: 'run:run-2',
    agent_id: 'agent-1',
    line: 'Sage could not finish the work',
    url: '/runs/run-2',
    at: NOW,
    run_id: 'run-2',
    channel_id: 'channel-1',
    failure_kind: 'tool_failed',
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
    /** The items of the daemon's Needs-You Queue, in its order. */
    queue?: NeedsYouItem[]
    failQueue?: boolean
    chief?: string | null
    report?: unknown
    writingRunId?: string | null
    /** The daemon holds the answer to a dismissal. */
    holdDismissals?: boolean
  } = {},
) {
  const post = vi.fn(async (path: string) =>
    options.holdDismissals === true && path.endsWith('/dismiss')
      ? new Promise<never>(() => undefined)
      : { data: { ok: true } },
  )
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
      if (path === '/api/v1/needs-you') {
        if (options.failQueue) throw new Error('Unavailable')
        const items = options.queue ?? []
        return { data: { items, count: items.length } }
      }
      if (path === '/api/v1/requests/{request_id}') {
        return { data: pendingRequest }
      }
      if (path === '/api/v1/runs') {
        const state = init?.params?.query?.state
        if (state === 'completed') return { data: { items: [run({ id: 'done-1' })] } }
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

it('does not report an empty queue when the queue fails to load', async () => {
  const options = { failQueue: true }
  const { api } = stubApi(options)
  mount(api)
  expect(await screen.findByText('Could not load your decisions')).toBeTruthy()
  expect(screen.queryByText('Nothing needs you.')).toBeNull()
  options.failQueue = false
  fireEvent.click(within(screen.getByRole('region', { name: 'Needs you' })).getByRole('button', { name: 'Try again' }))
  expect(await screen.findByText('Nothing needs you.')).toBeTruthy()
})

/** The row of the queue that shows `line`. */
async function queueRow(line: string): Promise<HTMLElement> {
  return (await screen.findByText(line)).closest('li') as HTMLElement
}

/** The names of the buttons of one row. */
function actions(row: HTMLElement): string[] {
  return within(row)
    .getAllByRole('button')
    .map((button) => button.textContent ?? '')
}

describe('Needs you', () => {
  it('says plainly when nothing needs the reader', async () => {
    const { api } = stubApi()
    mount(api)

    expect(await screen.findByText('Nothing needs you.')).toBeTruthy()
  })

  it('renders the items of the daemon in its order, each with its inline action', async () => {
    const { api } = stubApi({
      queue: [approval(), waiting(), keypad(), missedCall(), failure()],
    })
    mount(api)

    const section = screen.getByRole('region', { name: 'Needs you' })
    await within(section).findByText('Sage could not finish the work')
    const rows = within(section).getAllByRole('listitem')
    expect(rows.map((row) => row.getAttribute('data-kind'))).toEqual([
      'approval',
      'waiting',
      'keypad',
      'call',
      'failed',
    ])
    expect(within(section).getByText('5')).toBeTruthy()
    expect(within(rows[0]).getByText('Sage needs your approval')).toBeTruthy()
    expect(within(rows[0]).getByText('Open a file')).toBeTruthy()
    // The approval card reads its Request before it shows the decision.
    expect(await within(rows[0]).findByRole('button', { name: 'Approve' })).toBeTruthy()
    expect(within(rows[0]).getByRole('button', { name: 'Deny' })).toBeTruthy()
    expect(actions(rows[1])).toEqual(['Open conversation'])
    expect(actions(rows[2])).toEqual(['Clear the count'])
    expect(actions(rows[3])).toEqual(['Call back', 'Dismiss'])
    expect(actions(rows[4])).toEqual(['Open run', 'Dismiss'])
  })

  it('keeps the order of the daemon inside one kind', async () => {
    const { api } = stubApi({
      queue: [
        failure({ id: 'run:early', run_id: 'early', line: 'Nova could not finish the work', at: NOW - 60_000 }),
        failure({ id: 'run:late', run_id: 'late', line: 'Sage could not finish the work', at: NOW }),
      ],
    })
    mount(api)

    await screen.findByText('Sage could not finish the work')
    const lines = [...document.querySelectorAll('.home-queue-line')].map((line) => line.textContent)
    expect(lines).toEqual(['Nova could not finish the work', 'Sage could not finish the work'])
  })

  it('renders a pending approval with a working Approve', async () => {
    const { api, post } = stubApi({ queue: [approval()] })
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
    const { api, post } = stubApi({ queue: [approval()] })
    mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Deny' }))

    await waitFor(() =>
      expect(post).toHaveBeenCalledWith('/api/v1/requests/{request_id}/decision', {
        params: { path: { request_id: 'request-1' } },
        body: { decision: 'denied', scope: undefined, values: undefined },
      }),
    )
  })

  it('opens the conversation of a run that waits for the reader', async () => {
    const { api } = stubApi({ queue: [waiting()] })
    const opened = mount(api)

    fireEvent.click(await screen.findByRole('button', { name: 'Open conversation' }))
    expect(opened.channel).toEqual(['channel-1'])
  })

  it('draws a queue row in the one card frame, with one action in the footer', async () => {
    const { api } = stubApi({ queue: [waiting()] })
    mount(api)

    const line = await screen.findByText('Sage waits for your answer')
    const card = line.closest('.ui-frame')!
    expect(card.className).toContain('block-card')
    expect(card.className).toContain('home-queue-card')
    expect(card.getAttribute('data-kind')).toBe('waiting')
    expect(card.querySelector('.home-queue-dot')).not.toBeNull()
    // A run that waits shows its line and no caption.
    expect(card.querySelector('.home-queue-detail')).toBeNull()
    const footer = card.querySelector('.block-card-footer')!
    expect(footer.querySelectorAll('button')).toHaveLength(1)
    expect(footer.textContent).toContain('Open conversation')
  })

  it('lands a missed call on the agent DM with the call back message written', async () => {
    const { api } = stubApi({ queue: [missedCall()] })
    const opened = mount(api)

    expect(await screen.findByText('Sage missed a call from +14155550199')).toBeTruthy()
    expect(screen.getByText('Nobody answered.')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Call back' }))

    expect(opened.channel).toEqual(['channel-1'])
    expect(useComposerDraft.getState().byScope['channel-1']).toBe(
      'Please call +14155550199 back. They called and nobody answered.',
    )
  })

  it('says why a run failed, from the kind of the failure', async () => {
    const { api } = stubApi({ queue: [failure({ failure_kind: 'tool_failed' })] })
    mount(api)

    const row = await queueRow('Sage could not finish the work')
    expect(within(row).getByText('Ended because a tool failed')).toBeTruthy()
  })

  it('dismisses a failure when the reader opens its run', async () => {
    const { api, post } = stubApi({ queue: [failure()] })
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
    const { api, post } = stubApi({ queue: [failure()] })
    const opened = mount(api)

    const row = await queueRow('Sage could not finish the work')
    fireEvent.click(within(row).getByRole('button', { name: 'Dismiss' }))

    expect(opened.run).toEqual([])
    await waitFor(() =>
      expect(post).toHaveBeenCalledWith('/api/v1/runs/{run_id}/dismiss', {
        params: { path: { run_id: 'run-2' } },
      }),
    )
  })

  it('dismisses a missed call when the reader calls back, or dismisses it alone', async () => {
    const { api, post } = stubApi({
      queue: [
        missedCall(),
        missedCall({
          id: 'call:call-2',
          call_id: 'call-2',
          remote_e164: '+14155550188',
          line: 'Sage missed a call from +14155550188',
        }),
      ],
    })
    mount(api)

    fireEvent.click((await screen.findAllByRole('button', { name: 'Call back' }))[0])
    const second = await queueRow('Sage missed a call from +14155550188')
    fireEvent.click(within(second).getByRole('button', { name: 'Dismiss' }))

    await waitFor(() => {
      expect(post).toHaveBeenCalledWith('/api/v1/calls/{call_id}/dismiss', {
        params: { path: { call_id: 'call-1' } },
      })
      expect(post).toHaveBeenCalledWith('/api/v1/calls/{call_id}/dismiss', {
        params: { path: { call_id: 'call-2' } },
      })
    })
  })

  it('takes a dismissed item out of the queue before the daemon answers', async () => {
    const { api } = stubApi({ queue: [missedCall(), failure()], holdDismissals: true })
    mount(api)

    const call = await queueRow('Sage missed a call from +14155550199')
    fireEvent.click(within(call).getByRole('button', { name: 'Dismiss' }))
    const run = await queueRow('Sage could not finish the work')
    fireEvent.click(within(run).getByRole('button', { name: 'Dismiss' }))

    expect(await screen.findByText('Nothing needs you.')).toBeTruthy()
  })

  it('tells the reader when a keypad delay starts, and clears the count', async () => {
    // Noon, so the end of the delay falls on the same day and reads as a
    // clock time with no date.
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(new Date(2026, 0, 15, 12, 0))
    onTestFinished(() => { vi.useRealTimers() })
    const until = Date.now() + 60_000
    const { api, remove } = stubApi({
      queue: [keypad({ at: until, suspended_until: until })],
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
