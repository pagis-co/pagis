// The Forget section: one danger row that previews what goes
// before it removes anything.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { ConnectionForget } from './ConnectionForget'
import { asClient, stubApi } from './stub'

function mount(api = stubApi()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <ConnectionForget api={asClient(api)} connectionId="conn-1" />
    </QueryClientProvider>,
  )
  return api
}

describe('ConnectionForget', () => {
  it('says what a forget removes and what it does not', () => {
    mount()
    expect(screen.getByText('Forget everything imported from this account')).toBeTruthy()
    expect(screen.getByText(/Disconnecting alone does not/)).toBeTruthy()
  })

  it('previews the account target before it removes anything', async () => {
    const api = mount()
    await userEvent.click(screen.getByRole('button', { name: 'Preview what goes' }))
    await screen.findByRole('button', { name: 'Confirm forget' })
    expect(api.POST).toHaveBeenCalledWith('/api/v1/knowledge/forget/preview', {
      body: { target: { kind: 'account', connection_id: 'conn-1' } },
    })
  })
})
