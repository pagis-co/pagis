import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { CurrentWorkspaceContext } from '../../workspace'
import { WorkspaceNumber } from './WorkspaceNumber'

function mount(active: boolean, carrier = true) {
  let held = active
  const number = {
    id: 'number',
    e164: '+14155550123',
    status: 'active',
    registration: 'registered',
  }
  const chief = { id: 'chief', name: 'Pixie', standing_brief: 'Take a message.' }
  const api = {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/agents') return { data: { items: [chief] } }
      if (path === '/api/v1/workspace') {
        return { data: { id: 'workspace', chief_of_staff_agent_id: chief.id } }
      }
      if (path.endsWith('/available')) {
        return {
          data: {
            items: [{ e164: number.e164, monthly_cost: '1.00', currency: 'USD' }],
          },
        }
      }
      return {
        data: {
          number: held ? number : null,
          carrier: carrier ? { status: 'connected' } : null,
        },
      }
    }),
    POST: vi.fn(async () => {
      held = true
      return { data: number }
    }),
    DELETE: vi.fn(async () => {
      held = false
      return { response: new Response(null, { status: 204 }) }
    }),
  }
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  })
  render(
    <QueryClientProvider client={client}>
      <CurrentWorkspaceContext value="workspace">
        <WorkspaceNumber api={api as unknown as ApiClient} />
      </CurrentWorkspaceContext>
    </QueryClientProvider>,
  )
  return api
}

it('shows the Workspace Number and the Chief of Staff Standing Brief in Settings', async () => {
  mount(true)
  expect(await screen.findByText('+1 415 555 0123')).toBeTruthy()
  expect(screen.getByRole('button', { name: 'Release number' })).toBeTruthy()
  expect(screen.queryByRole('button', { name: 'Search' })).toBeNull()
  expect(await screen.findByLabelText('Standing Brief for the Chief of Staff, Pixie')).toBeTruthy()
})

it('searches and confirms a purchase for the Workspace', async () => {
  const api = mount(false)
  fireEvent.click(await screen.findByRole('button', { name: 'Search' }))
  fireEvent.click(await screen.findByRole('button', { name: /1 415 555 0123/ }))
  expect(api.POST).not.toHaveBeenCalled()
  expect(screen.getByText('1 · Search')).toBeTruthy()
  fireEvent.click(screen.getByRole('button', { name: 'Buy number' }))
  await waitFor(() =>
    expect(api.POST).toHaveBeenCalledWith('/api/v1/workspace/number', {
      body: { e164: '+14155550123' },
    }),
  )
  fireEvent.click(await screen.findByRole('button', { name: 'Done' }))
  expect(await screen.findByRole('button', { name: 'Release number' })).toBeTruthy()
  expect(screen.queryByRole('button', { name: 'Search' })).toBeNull()
})

it('confirms a release and shows the number search again', async () => {
  const api = mount(true)
  fireEvent.click(await screen.findByRole('button', { name: 'Release number' }))
  expect(api.DELETE).not.toHaveBeenCalled()
  fireEvent.click(screen.getByRole('button', { name: 'Release it' }))
  await waitFor(() =>
    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/workspace/number'),
  )
  expect(await screen.findByRole('button', { name: 'Search' })).toBeTruthy()
})

it('shows the carrier registration and no build status', async () => {
  mount(true)
  expect(await screen.findByText('Registered with the carrier')).toBeTruthy()
  expect(screen.queryByText(/Not built/)).toBeNull()
})

it('explains when no carrier is connected', async () => {
  mount(false, false)
  expect(await screen.findByTestId('no-carrier')).toBeTruthy()
  expect(screen.queryByRole('button', { name: 'Search' })).toBeNull()
})

it('searches for a city through the carrier', async () => {
  const api = mount(false)
  fireEvent.change(await screen.findByLabelText('Area code or city'), {
    target: { value: 'Boston' },
  })
  fireEvent.click(screen.getByRole('button', { name: 'Search' }))
  await waitFor(() =>
    expect(api.GET).toHaveBeenCalledWith('/api/v1/settings/phone-numbers/available', {
      params: { query: { country: 'US', area_code: undefined, locality: 'Boston' } },
    }),
  )
})
