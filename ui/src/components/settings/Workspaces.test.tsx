import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { Workspaces } from './Workspaces'

it('lists each Workspace and saves and clears its Spend Cap', async () => {
  const api = {
    GET: vi.fn(async (path: string) => path === '/api/v1/administration/people' ? { data: { items: [] } } : ({ data: { items: [{ id: 'ws-1', name: 'Private work', person: { name: 'Grace', email: 'grace@example.com' }, total: { cost_usd: 12 }, monthly_spend_cap_usd: 25 }] } })),
    PUT: vi.fn(async () => ({ data: {} })),
  }
  render(<QueryClientProvider client={new QueryClient()}><Workspaces api={api as unknown as ApiClient} /></QueryClientProvider>)
  expect(await screen.findByText('Private work')).toBeTruthy()
  expect(screen.getByText(/Grace/)).toBeTruthy()
  expect(screen.getByText('$12.00 this month')).toBeTruthy()
  const input = screen.getByLabelText('Monthly spend cap for Private work')
  await userEvent.clear(input)
  await userEvent.type(input, '-1')
  expect(screen.getByRole('button', { name: 'Save the spend cap for Private work' }).hasAttribute('disabled')).toBe(true)
  for (const value of ['0', '40', '']) {
    await userEvent.clear(input)
    if (value) await userEvent.type(input, value)
    await userEvent.click(screen.getByRole('button', { name: 'Save the spend cap for Private work' }))
    await waitFor(() => expect(api.PUT).toHaveBeenCalledWith('/api/v1/administration/workspaces/{workspace_id}/spend-cap', {
      params: { path: { workspace_id: 'ws-1' } }, body: { monthly_spend_cap_usd: value ? Number(value) : null },
    }))
  }
})

it('creates a Shared Workspace with an active first Manager', async () => {
  const manager = { id: 'sam', name: 'Sam', disabled: false }
  const api = {
    GET: vi.fn(async (path: string) => ({ data: { items: path.endsWith('/people') ? [manager] : [] } })),
    POST: vi.fn(async () => ({ data: { id: 'shared' } })),
  }
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Workspaces api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
  fireEvent.change(screen.getByLabelText('Shared Workspace name'), { target: { value: ' Support ' } })
  fireEvent.click(screen.getByRole('combobox', { name: 'First Manager' }))
  fireEvent.click(await screen.findByRole('option', { name: 'Sam' }))
  fireEvent.click(screen.getByRole('button', { name: 'Create Shared Workspace' }))
  await waitFor(() => expect(api.POST).toHaveBeenCalledWith('/api/v1/administration/workspaces', {
    body: { name: 'Support', manager_id: 'sam' },
  }))
})
