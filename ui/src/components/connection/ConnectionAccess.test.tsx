// The Access section: one row per Agent with its grants as
// chips. Grant gives the account's access; Change saves each toggle.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { ConnectionAccess } from './ConnectionAccess'
import { asClient, clown, connected, sage, sageGrant, stubApi } from './stub'

function mount(api = stubApi()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <ConnectionAccess
        api={asClient(api)}
        connection={connected()}
        agents={[sage, clown, { ...clown, id: 'ag3', name: 'Gone', status: 'archived' }]}
        grants={[sageGrant]}
      />
    </QueryClientProvider>,
  )
  return api
}

describe('ConnectionAccess', () => {
  it('shows one row per live Agent with its grants as chips', () => {
    mount()
    expect(screen.getByText('Sage')).toBeTruthy()
    expect(screen.getByText('Clown')).toBeTruthy()
    expect(screen.queryByText('Gone')).toBeNull()
    const sageRow = screen.getByText('Sage').closest('.ui-row')!
    expect(sageRow.textContent).toContain('Read Gmail')
    expect(sageRow.textContent).toContain('Send email')
    expect(sageRow.textContent).toContain('Read Calendar')
    expect(screen.getByText('no access')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Change access for Sage' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Grant to Clown' })).toBeTruthy()
  })

  it("grants an Agent the account's access", async () => {
    const api = mount()
    fireEvent.click(screen.getByRole('button', { name: 'Grant to Clown' }))
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/grants', {
        body: {
          agent_id: 'ag2',
          connection_id: 'conn-1',
          capabilities: ['gmail_read', 'gmail_send', 'calendar_read'],
        },
      }),
    )
  })

  it('saves each capability toggle at once', async () => {
    const api = mount()
    fireEvent.click(screen.getByRole('button', { name: 'Change access for Sage' }))
    fireEvent.click(screen.getByLabelText('Send email'))
    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/grants/{grant_id}/capabilities', {
        params: { path: { grant_id: 'grant-1' } },
        body: { capabilities: ['gmail_read', 'calendar_read'] },
      }),
    )
  })

  it('removes an Agent from the account', async () => {
    const api = mount()
    fireEvent.click(screen.getByRole('button', { name: 'Remove access for Sage' }))
    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/grants/{grant_id}', {
        params: { path: { grant_id: 'grant-1' } },
      }),
    )
  })
})
