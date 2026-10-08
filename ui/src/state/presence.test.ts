// The presence store: the rings follow the run and call frames,
// the caption follows the trigger, and the unread dot follows the
// channel the reader has open.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderHook, waitFor } from '@testing-library/react'
import { createElement } from 'react'
import type { ReactNode } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient, EventRow } from '../api/client'
import {
  LIVE_RUN_STATES,
  agentPresence,
  channelCaption,
  channelPresence,
  isUnread,
  liveCallOf,
  usePresence,
  usePresenceSeed,
} from './presence'

function event(overrides: Partial<EventRow>): EventRow {
  return {
    id: 'ev-1',
    event_type: 'run.state_changed',
    agent_id: 'ag-1',
    channel_id: 'ch-1',
    run_id: 'run-1',
    payload: {},
    ...overrides,
  }
}

function apply(type: string, overrides: Partial<EventRow> = {}): void {
  usePresence.getState().applyFrame(type, event({ ...overrides, event_type: type }))
}

function presenceOf(agentId = 'ag-1') {
  return agentPresence(usePresence.getState(), agentId)
}

describe('usePresence', () => {
  beforeEach(() => {
    usePresence.setState({
      runs: {},
      onCall: {},
      unread: {},
      selectedChannelId: null,
      seeded: false,
    })
  })

  it('turns the ring to working, to needs you, and back to idle', () => {
    expect(presenceOf()).toBe('idle')

    apply('run.created', { payload: { trigger_kind: 'message' } })
    apply('run.state_changed', { payload: { to: 'running' } })
    expect(presenceOf()).toBe('working')

    // Reflection settles memory after the work ends (ADR-0002). The
    // row says so in words; the ring is for work the user waits on.
    apply('run.state_changed', { payload: { to: 'reflecting' } })
    expect(presenceOf()).toBe('idle')
    expect(channelCaption(usePresence.getState(), 'ch-1')).toBe('Updating memory')

    apply('run.state_changed', { payload: { to: 'waiting_for_user' } })
    expect(presenceOf()).toBe('waiting')

    apply('run.state_changed', { payload: { to: 'waiting_for_approval' } })
    expect(presenceOf()).toBe('waiting')

    apply('run.state_changed', { payload: { to: 'completed' } })
    expect(presenceOf()).toBe('idle')
  })

  // A synced arrival reflects memory in the background and has no
  // channel (ADR-0011). The user calls none of it working.
  it('leaves the ring idle while a background run settles memory', () => {
    apply('run.created', { channel_id: null, payload: { trigger_kind: 'arrival' } })
    apply('run.state_changed', { channel_id: null, payload: { to: 'running' } })
    expect(presenceOf()).toBe('idle')

    // An approval still needs the user, wherever the run works.
    apply('run.state_changed', { channel_id: null, payload: { to: 'waiting_for_approval' } })
    expect(presenceOf()).toBe('waiting')
  })

  it('gives one channel the ring of the work done in it', () => {
    const state = () => usePresence.getState()
    apply('run.created', { payload: { trigger_kind: 'message' } })
    apply('run.state_changed', { payload: { to: 'running' } })

    expect(channelPresence(state(), 'ch-1', 'ag-1')).toBe('working')
    expect(channelPresence(state(), 'ch-2', 'ag-1')).toBe('idle')
    expect(channelPresence(state(), 'ch-1', 'ag-2')).toBe('idle')
  })

  it('reads a call over a run, and gives the ring back when it ends', () => {
    apply('run.state_changed', { payload: { to: 'running' } })
    apply('call.placed', { channel_id: null, payload: { call_id: 'call-1' } })
    expect(presenceOf()).toBe('oncall')

    apply('call.ended', { channel_id: null })
    expect(presenceOf()).toBe('working')
  })

  // The Thread header joins the call the Agent is on, so the
  // store keeps the Call the ring stands for.
  it('names the call the Agent is on', () => {
    apply('call.placed', { channel_id: null, payload: { call_id: 'call-1' } })
    expect(liveCallOf(usePresence.getState(), 'ag-1')).toBe('call-1')

    apply('call.ended', { channel_id: null, payload: { call_id: 'call-1' } })
    expect(liveCallOf(usePresence.getState(), 'ag-1')).toBeNull()
  })

  it('captions the row from the run trigger and drops it when the run ends', () => {
    apply('run.created', { payload: { trigger_kind: 'schedule' } })
    expect(channelCaption(usePresence.getState(), 'ch-1')).toBe(
      'Working on a schedule',
    )

    apply('run.state_changed', { payload: { to: 'running' } })
    expect(channelCaption(usePresence.getState(), 'ch-1')).toBe(
      'Working on a schedule',
    )

    apply('run.state_changed', { payload: { to: 'failed' } })
    expect(channelCaption(usePresence.getState(), 'ch-1')).toBeNull()
  })

  it('marks an unread channel and clears it on select', () => {
    usePresence.getState().select('ch-2')

    apply('message.completed', {
      channel_id: 'ch-1',
      payload: { author_kind: 'agent' },
    })
    expect(isUnread(usePresence.getState(), 'ch-1')).toBe(true)

    usePresence.getState().select('ch-1')
    expect(isUnread(usePresence.getState(), 'ch-1')).toBe(false)
  })

  it('leaves the open channel and the reader’s own messages unread-free', () => {
    usePresence.getState().select('ch-1')

    apply('message.completed', {
      channel_id: 'ch-1',
      payload: { author_kind: 'agent' },
    })
    expect(isUnread(usePresence.getState(), 'ch-1')).toBe(false)

    apply('message.completed', {
      channel_id: 'ch-2',
      payload: { author_kind: 'user' },
    })
    expect(isUnread(usePresence.getState(), 'ch-2')).toBe(false)
  })

  it('seeds the unfinished runs once, and never fights a later list', () => {
    usePresence.getState().seed([
      {
        id: 'run-9',
        agent_id: 'ag-9',
        channel_id: 'ch-9',
        title: 'Book the Austin trip',
        trigger_kind: 'message',
        hop_count: 0,
        state: 'running',
        created_at: 1,
      },
    ])
    expect(presenceOf('ag-9')).toBe('working')

    usePresence.getState().applyFrame(
      'run.state_changed',
      event({ run_id: 'run-9', agent_id: 'ag-9', payload: { to: 'completed' } }),
    )
    usePresence.getState().seed([
      {
        id: 'run-9',
        agent_id: 'ag-9',
        channel_id: 'ch-9',
        title: 'Book the Austin trip',
        trigger_kind: 'message',
        hop_count: 0,
        state: 'running',
        created_at: 1,
      },
    ])
    expect(presenceOf('ag-9')).toBe('idle')
  })
})

describe('usePresenceSeed', () => {
  beforeEach(() => {
    usePresence.setState({
      runs: {},
      onCall: {},
      unread: {},
      selectedChannelId: null,
      seeded: false,
    })
  })

  it('asks for every live state in one request and marks the rings', async () => {
    const GET = vi.fn(async () => ({
      data: {
        items: [
          {
            id: 'run-1',
            agent_id: 'ag-work',
            channel_id: 'ch-1',
            title: 'Book the Austin trip',
            trigger_kind: 'message',
            hop_count: 0,
            state: 'running',
            created_at: 1,
          },
          {
            id: 'run-2',
            agent_id: 'ag-needs-you',
            channel_id: 'ch-2',
            title: 'Book the Austin trip',
            trigger_kind: 'message',
            hop_count: 0,
            state: 'waiting_for_approval',
            created_at: 2,
          },
        ],
      },
    }))
    const api = { GET } as unknown as ApiClient
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    })
    const wrapper = ({ children }: { children: ReactNode }) =>
      createElement(QueryClientProvider, { client }, children)

    renderHook(() => usePresenceSeed(api), { wrapper })

    await waitFor(() => expect(usePresence.getState().seeded).toBe(true))
    expect(GET).toHaveBeenCalledTimes(1)
    expect(GET).toHaveBeenCalledWith('/api/v1/runs', {
      params: { query: { state: LIVE_RUN_STATES.join(',') } },
    })
    expect(presenceOf('ag-work')).toBe('working')
    expect(presenceOf('ag-needs-you')).toBe('waiting')
  })
})
