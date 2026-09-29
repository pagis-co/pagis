// The sync line of a Connections card.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { SyncLine } from './SyncLine'
import { asClient, stubApi } from './stub'

function mount(api = stubApi()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const view = render(
    <QueryClientProvider client={client}>
      <SyncLine api={asClient(api)} connectionId="conn-1" />
    </QueryClientProvider>,
  )
  return view
}

describe('SyncLine', () => {
  it('says how much is acquired, when, and how many rules decide', async () => {
    mount()
    expect(await screen.findByText('Sync: 12,480 pages · last 3 min ago')).toBeTruthy()
    expect(screen.getByText('Reflection filter: 2 rules')).toBeTruthy()
  })

  it('shows nothing for an account that does not sync', () => {
    const { container } = mount(stubApi({ sync: null }))
    expect(container.textContent).toBe('')
  })
})
