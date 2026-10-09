// Home on the phone (ADR-0034): the newest approval is a card with Deny
// and Approve, every other item of the Needs-You Queue is a row with its
// line and one line of detail, and the Work record reads under the brief.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient, NeedsYouItem } from '../../api/client'
import { useComposerDraft } from '../../state/composerDraft'
import { renderInRouter } from '../../test/router'
import { threadScope } from '../../timeline'
import { Home } from './Home'
import { callBackDraft } from './queue'

const NOW = Date.now()

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
    title: 'Send the October invoice',
    body: 'Email to billing@example.com',
    ...fields,
  }
}

const waiting: Item<'waiting'> = {
  kind: 'waiting',
  id: 'run:run-7',
  agent_id: 'agent-2',
  line: 'Nova waits for your answer',
  url: '/c/channel-2',
  at: NOW,
  run_id: 'run-7',
  channel_id: 'channel-2',
}

const keypad: Item<'keypad'> = {
  kind: 'keypad',
  id: 'keypad',
  line: 'Callers entered a wrong keypad code 6 times',
  url: '/',
  at: NOW,
  failed_attempts: 6,
  suspended_until: NOW + 60_000,
}

const missedCall: Item<'call'> = {
  kind: 'call',
  id: 'call:call-1',
  agent_id: 'agent-1',
  line: 'Sage missed a call',
  url: '/calls/call-1',
  at: NOW,
  call_id: 'call-1',
  remote_e164: '+14155550199',
  left_message: true,
}

const failure: Item<'failed'> = {
  kind: 'failed',
  id: 'run:run-2',
  agent_id: 'agent-1',
  line: 'Sage could not finish the work',
  url: '/runs/run-2',
  at: NOW,
  run_id: 'run-2',
  channel_id: 'channel-1',
  failure_kind: 'tool_failed',
}

function stubApi(queue: NeedsYouItem[]) {
  const post = vi.fn(async () => ({ data: { ok: true } }))
  const remove = vi.fn(async () => ({ error: undefined, response: { ok: true, status: 204 } }))
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
              {
                id: 'channel-2',
                title: 'Nova',
                kind: 'dm',
                agent_ids: ['agent-2'],
                user_member: true,
                last_message: {
                  author_kind: 'agent',
                  author_agent_id: 'agent-2',
                  created_at: NOW,
                  text_content: 'Which flight do you want on Tuesday?',
                },
              },
            ],
          },
        }
      }
      if (path === '/api/v1/workspace') {
        return { data: { id: 'ws-1', name: 'Workspace', timezone: 'UTC', chief_of_staff_agent_id: 'agent-1' } }
      }
      if (path === '/api/v1/workspace/report') {
        return { data: { schedule_id: null, next_due_at: null, message: null, writing_run_id: null } }
      }
      if (path === '/api/v1/needs-you') return { data: { items: queue, count: queue.length } }
      if (path === '/api/v1/requests/{request_id}') {
        return { data: { id: 'request-1', state: 'pending', payload: { tool_name: 'mail__send' } } }
      }
      if (path === '/api/v1/runs' && init?.params?.query?.state === 'completed') {
        return {
          data: {
            items: [
              {
                id: 'done-1',
                agent_id: 'agent-2',
                channel_id: 'channel-2',
                root_message_id: null,
                title: 'Filed the September receipts',
                trigger_kind: 'schedule',
                trigger_ref: null,
                hop_count: 0,
                state: 'completed',
                error: null,
                started_at: NOW,
                ended_at: NOW,
                created_at: NOW,
                duration_ms: 240_000,
              },
            ],
          },
        }
      }
      return { data: { items: [] } }
    }),
    POST: post,
    DELETE: remove,
  }
  return { api: api as unknown as ApiClient, post, remove }
}

function mount(api: ApiClient) {
  const opened = { channel: [] as string[], run: [] as string[] }
  const history = renderInRouter(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <Home
        api={api}
        onOpenChannel={(id) => opened.channel.push(id)}
        onOpenRun={(id) => opened.run.push(id)}
      />
    </QueryClientProvider>,
  )
  return { opened, history }
}

/** The row of the queue that holds `line`. */
async function queueRow(line: string): Promise<HTMLElement> {
  return (await screen.findByText(line)).closest('.home-phone-swipe') as HTMLElement
}

beforeEach(() => {
  useComposerDraft.setState({ byScope: {} })
  vi.stubGlobal('matchMedia', (query: string) => ({
    matches: query.includes('max-width'),
    media: query,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  }))
})
afterEach(() => vi.unstubAllGlobals())

describe('Home on the phone', () => {
  it('draws the newest approval as the card and an older one as a row', async () => {
    const { api } = stubApi([
      approval({ id: 'request:old', request_id: 'old', at: NOW - 60_000, title: 'Pay the hotel' }),
      approval(),
    ])
    mount(api)

    const card = await screen.findByTestId('approval-card')
    expect(within(card).getByText('Send the October invoice')).toBeTruthy()
    expect(within(card).getByRole('button', { name: 'Deny' })).toBeTruthy()
    expect(within(card).queryByText('Pay the hotel')).toBeNull()
    expect(await queueRow('Pay the hotel')).toBeTruthy()
  })

  it('approves the card once', async () => {
    const { api, post } = stubApi([approval()])
    mount(api)

    fireEvent.click(await within(await screen.findByTestId('approval-card')).findByRole('button', { name: 'Approve' }))

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    expect(post).toHaveBeenCalledWith('/api/v1/requests/{request_id}/decision', {
      params: { path: { request_id: 'request-1' } },
      body: { decision: 'approved', scope: 'once', values: undefined },
    })
  })

  it('opens the Run of a failed row', async () => {
    const { api } = stubApi([failure])
    const { opened } = mount(api)

    fireEvent.click(within(await queueRow('Sage could not finish the work')).getByRole('button'))

    expect(opened.run).toEqual(['run-2'])
  })

  it('shows Dismiss when a failed row is swiped left', async () => {
    const { api } = stubApi([failure])
    mount(api)
    const row = await queueRow('Sage could not finish the work')

    fireEvent.pointerDown(row, { clientX: 300 })
    fireEvent.pointerUp(row, { clientX: 200 })

    expect(within(row).getByRole('button', { name: 'Dismiss' })).toBeTruthy()
  })

  it('shows the question of a waiting row as its detail', async () => {
    const { api } = stubApi([waiting])
    mount(api)

    const row = await queueRow('Nova waits for your answer')
    expect(await within(row).findByText('Which flight do you want on Tuesday?')).toBeTruthy()
  })

  it('clears the keypad count from a control that says so', async () => {
    const { api, remove } = stubApi([keypad])
    mount(api)

    const row = await queueRow('Callers entered a wrong keypad code 6 times')
    fireEvent.click(within(row).getByRole('button', { name: 'Clear the count' }))

    await waitFor(() => expect(remove).toHaveBeenCalledWith('/api/v1/settings/keypad-code/failures'))
  })

  it('writes the call-back message from the call button of a missed call', async () => {
    const { api } = stubApi([missedCall])
    const { opened } = mount(api)

    fireEvent.click(within(await queueRow('Sage missed a call')).getByRole('button', { name: 'Call back' }))

    expect(useComposerDraft.getState().byScope[threadScope('channel-1')]).toBe(
      callBackDraft({ remote_e164: '+14155550199', left_message: true }),
    )
    expect(opened.channel).toEqual(['channel-1'])
  })

  it('shows the Run title in the Work record', async () => {
    const { api } = stubApi([])
    mount(api)

    const record = await screen.findByRole('region', { name: 'Work record' })
    expect(await within(record).findByText('Filed the September receipts')).toBeTruthy()
  })
})
