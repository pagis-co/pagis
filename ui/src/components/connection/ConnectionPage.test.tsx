// The connection page: the head and the four sections, each of
// which saves on its own.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { ConnectionPage } from './ConnectionPage'
import { asClient, filter, status, stubApi } from './stub'

function mount(api = stubApi(), connectionId = 'conn-1') {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const onBack = vi.fn()
  render(
    <QueryClientProvider client={client}>
      <ConnectionPage api={asClient(api)} connectionId={connectionId} onBack={onBack} />
    </QueryClientProvider>,
  )
  return { api, onBack }
}

describe('ConnectionPage', () => {
  it('shows the head and the four sections', async () => {
    mount()
    expect(
      await screen.findByRole('heading', { name: 'Google · alice@example.com' }),
    ).toBeTruthy()
    for (const label of ['Access', 'Sync', 'What reflects', 'Forget']) {
      expect(screen.getByText(label)).toBeTruthy()
    }
    expect(await screen.findByText('Read Gmail')).toBeTruthy()
    expect(await screen.findByText('Up to date')).toBeTruthy()
    expect(screen.getByText(/when Labels has/)).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Preview what goes' })).toBeTruthy()
  })

  it('saves a rule change at once, with the sync settings kept', async () => {
    const { api } = mount()
    fireEvent.click(
      await screen.findByRole('button', { name: 'Change the default verdict' }),
    )
    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/connections/{connection_id}/sync', {
        params: { path: { connection_id: 'conn-1' } },
        body: {
          agent_id: status.config.agent_id,
          enabled: true,
          since: status.config.since,
          filter: { ...filter, default: 'reflect' },
        },
      }),
    )
  })

  it('says so when the address names no connection', async () => {
    const { onBack } = mount(stubApi(), 'gone')
    fireEvent.click(await screen.findByRole('button', { name: 'Back to Connections' }))
    expect(onBack).toHaveBeenCalled()
  })
})
