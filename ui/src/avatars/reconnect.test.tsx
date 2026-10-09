import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, render, screen, waitFor } from '@testing-library/react'
import { expect, it } from 'vitest'
import type { ApiClient } from '../api/client'
import { agentPresence, usePresence, usePresenceSeed } from '../state/presence'

it('restores live presence after a resync even when the API returns the same runs', async () => {
  const runs = [
    {
      id: 'run',
      agent_id: 'sage',
      channel_id: 'conversation',
      state: 'running',
      title: 'Book the Austin trip',
      trigger_kind: 'message',
    },
  ]
  const api = {
    GET: async () => ({ data: { items: runs } }),
  } as unknown as ApiClient
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  usePresence.setState({ runs: {}, seeded: false, onCall: {} })
  function View() {
    usePresenceSeed(api)
    const presence = usePresence((state) => agentPresence(state, 'sage'))
    return <p role="status">{presence}</p>
  }
  const view = render(
    <QueryClientProvider client={client}>
      <View />
    </QueryClientProvider>,
  )
  await waitFor(() =>
    expect(screen.getByRole('status').textContent).toBe('working'),
  )
  await act(async () => {
    usePresence.setState({ runs: {}, seeded: false })
    await client.invalidateQueries()
  })
  await waitFor(() =>
    expect(screen.getByRole('status').textContent).toBe('working'),
  )
  view.unmount()
  client.clear()
  usePresence.setState({ runs: {}, seeded: false })
})
