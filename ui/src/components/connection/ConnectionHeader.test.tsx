// The head of a connection page: the provider and the account,
// the name Agents use, the state badge, Reconnect and Disconnect.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { ConnectionHeader } from './ConnectionHeader'
import { asClient, connected, stubApi } from './stub'

function mount(api = stubApi(), onBack = vi.fn(), connection = connected()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <ConnectionHeader api={asClient(api)} connection={connection} onBack={onBack} />
    </QueryClientProvider>,
  )
  return { api, onBack }
}

describe('ConnectionHeader', () => {
  it('names the provider, the account, the short name and the state', () => {
    mount()
    expect(screen.getByRole('heading', { name: 'Google · alice@example.com' })).toBeTruthy()
    expect(screen.getByText(/Sprites know it as/).textContent).toContain('personal')
    expect(screen.getByRole('status', { name: 'Connected' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Back to Connections' })).toBeTruthy()
  })

  it('reconnects with the access the account already has', async () => {
    const { api } = mount()
    fireEvent.click(screen.getByRole('button', { name: 'Reconnect' }))
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/settings/connections/{connection_id}/authorize',
        {
          params: { path: { connection_id: 'conn-1' } },
          body: { capabilities: ['gmail_read', 'gmail_send', 'calendar_read'], api_key: undefined },
        },
      ),
    )
  })

  it('opens the start route that a Google reconnect answers', async () => {
    const open = vi.fn()
    vi.stubGlobal('open', open)
    const api = stubApi()
    const start = 'https://pagis.example.net/api/v1/connections/google/start?state=abc'
    api.POST.mockImplementation(async () => ({
      data: { connection: connected({ status: 'connecting' }), authorization_url: start },
    }))
    mount(api)

    fireEvent.click(screen.getByRole('button', { name: 'Reconnect' }))

    await waitFor(() => expect(open).toHaveBeenCalledWith(start, '_blank', 'noopener'))
    vi.unstubAllGlobals()
  })

  it('disconnects and returns to the list', async () => {
    const { api, onBack } = mount()
    fireEvent.click(screen.getByRole('button', { name: 'Disconnect' }))
    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/settings/connections/{connection_id}', {
        params: { path: { connection_id: 'conn-1' } },
      }),
    )
    await waitFor(() => expect(onBack).toHaveBeenCalled())
  })
})
