import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { Workspaces } from './Workspaces'

it('lists each Workspace and saves and clears its Spend Cap', async () => {
  const api = {
    GET: vi.fn(async (path: string) => (path === '/api/v1/administration/people' || path === '/api/v1/admin/audit') ? { data: { items: [] } } : ({ data: { items: [{ id: 'ws-1', name: 'Private work', person: { name: 'Grace', email: 'grace@example.com' }, total: { cost_usd: 12 }, monthly_spend_cap_usd: 25 }] } })),
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

it('requires a reason, starts and ends a Legal Hold, and reads the Audit Log', async () => {
  let held = false
  const api = {
    GET: vi.fn(async (path: string) => ({ data: { items: path.endsWith('/people') ? [] : path.endsWith('/audit') ? [{ id: 'audit-1', action: 'legal_hold.started', subject_workspace_id: 'ws-1', actor: { kind: 'person', person_id: 'sam' }, reason: 'Preserve records', at: 100 }] : [{ id: 'ws-1', name: 'Support', managers: [], total: { cost_usd: 0 }, monthly_spend_cap_usd: null, legal_hold: held ? { started_at: 100, started_by: { kind: 'person', person_id: 'sam' } } : null }] } })),
    PUT: vi.fn(async () => { held = true; return { data: {} } }),
    DELETE: vi.fn(async () => { held = false; return { data: {} } }),
  }
  render(<QueryClientProvider client={new QueryClient()}><Workspaces api={api as unknown as ApiClient} /></QueryClientProvider>)
  expect(await screen.findByText('Support')).toBeTruthy()
  const start = screen.getByRole('button', { name: 'Start Legal Hold for Support' })
  expect(start.hasAttribute('disabled')).toBe(true)
  await userEvent.type(screen.getByLabelText('Legal Hold reason for Support'), 'Preserve records')
  await userEvent.click(start)
  await waitFor(() => expect(api.PUT).toHaveBeenCalledWith('/api/v1/admin/workspaces/{workspace_id}/legal-hold', {
    params: { path: { workspace_id: 'ws-1' } }, body: { reason: 'Preserve records' },
  }))
  expect(await screen.findByText('Under Legal Hold')).toBeTruthy()
  expect(screen.getByText('Audit Log')).toBeTruthy()
  expect(await screen.findByText('Preserve records')).toBeTruthy()
  await userEvent.click(screen.getByRole('button', { name: 'End Legal Hold for Support' }))
  await waitFor(() => expect(api.DELETE).toHaveBeenCalledWith('/api/v1/admin/workspaces/{workspace_id}/legal-hold', {
    params: { path: { workspace_id: 'ws-1' } },
  }))
})

it('saves the Shared Workspace Spend Cap through its own route beside the limits', async () => {
  const api = {
    GET: vi.fn(async (path: string) => path.endsWith('/limits') ? { data: { permitted_plugins: null, permitted_connections: null, max_mail_visibility: 'all_mail', outside_plugins: 0, outside_connections: 0 } } : { data: { items: path.endsWith('/workspaces') ? [{ id: 'team', name: 'Support', kind: 'shared', managers: [], monthly_spend_cap_usd: 25, total: { cost_usd: 0 } }] : [] } }),
    PUT: vi.fn(async () => ({ data: {} })),
  }
  render(<QueryClientProvider client={new QueryClient()}><Workspaces api={api as unknown as ApiClient} /></QueryClientProvider>)
  const cap = await screen.findByLabelText('Monthly spend cap for Support')
  fireEvent.change(cap, { target: { value: '40' } })
  fireEvent.click(screen.getByRole('button', { name: 'Save the spend cap for Support' }))
  await waitFor(() => expect(api.PUT).toHaveBeenCalledWith('/api/v1/administration/workspaces/{workspace_id}/spend-cap', { params: { path: { workspace_id: 'team' } }, body: { monthly_spend_cap_usd: 40 } }))
  expect(await screen.findByRole('button', { name: 'Save limits for Support' })).toBeTruthy()
})
