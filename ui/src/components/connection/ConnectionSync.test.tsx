// The Sync section: the status strip, then the agent and the
// start date. Each control saves at once.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { ConnectionSync } from './ConnectionSync'
import { asClient, clown, sage, status, stubApi } from './stub'

function mount(api = stubApi()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <ConnectionSync api={asClient(api)} connectionId="conn-1" agents={[sage, clown]} />
    </QueryClientProvider>,
  )
  return api
}

const saved = {
  agent_id: 'ag1',
  enabled: true,
  since: status.config.since,
  filter: status.config.filter,
}

describe('ConnectionSync', () => {
  it('reads the state, the acquired pages, the last arrival and today', async () => {
    mount()
    expect(await screen.findByText('Up to date')).toBeTruthy()
    expect(screen.getByText('Acquired')).toBeTruthy()
    expect(screen.getByText('12,480 messages')).toBeTruthy()
    expect(screen.getByText('Last arrival')).toBeTruthy()
    expect(screen.getByText('3 min ago')).toBeTruthy()
    expect(screen.getByText('Arrivals today')).toBeTruthy()
    expect(screen.getByText('41 handled · 2 skipped')).toBeTruthy()
  })

  it('pauses the sync and keeps the other settings', async () => {
    const api = mount()
    fireEvent.click(await screen.findByRole('button', { name: 'Pause sync' }))
    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/connections/{connection_id}/sync', {
        params: { path: { connection_id: 'conn-1' } },
        body: { ...saved, enabled: false },
      }),
    )
  })

  it('saves the responsible agent as it changes', async () => {
    const user = userEvent.setup()
    const api = mount()
    const select = await screen.findByRole('combobox', { name: 'Responsible sprite' })
    expect(select.textContent).toContain('Sage')
    select.focus()
    await user.keyboard('{Enter}')
    await user.keyboard('{ArrowDown}{Enter}')
    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/connections/{connection_id}/sync', {
        params: { path: { connection_id: 'conn-1' } },
        body: { ...saved, agent_id: 'ag2' },
      }),
    )
  })

  it('saves the date the history starts at', async () => {
    const api = mount()
    const date = await screen.findByLabelText('Import history from')
    expect((date as HTMLInputElement).value).toBe('2025-08-09')
    fireEvent.change(date, { target: { value: '2025-09-01' } })
    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/connections/{connection_id}/sync', {
        params: { path: { connection_id: 'conn-1' } },
        body: { ...saved, since: Date.parse('2025-09-01') },
      }),
    )
  })

  it('names the state when the sync is paused or needs attention', async () => {
    mount(stubApi({ sync: { ...status, config: { ...status.config, enabled: false } } }))
    expect(await screen.findByText('Paused')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Resume sync' })).toBeTruthy()
  })
})
