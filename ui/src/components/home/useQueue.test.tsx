// The Needs-You Queue as one hook: Home and the sidebar read
// the same count from the same four queries and the presence store.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { usePresence } from '../../state/presence'
import { useQueue } from './useQueue'

function stubApi(pending: number) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/requests') {
        return {
          data: {
            items: Array.from({ length: pending }, (_, index) => ({
              id: `req-${index}`,
              agent_id: 'ag-1',
              kind: 'tool_action',
              state: 'pending',
              payload: {},
              created_at: index,
            })),
          },
        }
      }
      return { data: { items: [] } }
    }),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return renderHook(() => useQueue(api as unknown as ApiClient), {
    wrapper: ({ children }) => (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    ),
  })
}

describe('useQueue', () => {
  beforeEach(() => {
    usePresence.setState({
      runs: {},
      onCall: {},
      unread: {},
      selectedChannelId: null,
      seeded: false,
    })
  })

  it('is pending until the four lists land, then counts the pending Requests', async () => {
    const { result } = mount(stubApi(2))
    expect(result.current.isPending).toBe(true)
    await waitFor(() => expect(result.current.isPending).toBe(false))
    expect(result.current.items.map((item) => item.kind)).toEqual(['approval', 'approval'])
  })

  it('counts a run that waits for the user without a request', async () => {
    const base = { agent_id: 'ag-1', channel_id: 'ch-1', run_id: 'run-1' }
    usePresence.getState().applyFrame('run.created', {
      ...base,
      id: 'ev-1',
      event_type: 'run.created',
      payload: { trigger_kind: 'message' },
    })
    usePresence.getState().applyFrame('run.state_changed', {
      ...base,
      id: 'ev-2',
      event_type: 'run.state_changed',
      payload: { to: 'waiting_for_user' },
    })
    const { result } = mount(stubApi(0))
    await waitFor(() => expect(result.current.isPending).toBe(false))
    expect(result.current.items.map((item) => item.kind)).toEqual(['waiting'])
  })
})
