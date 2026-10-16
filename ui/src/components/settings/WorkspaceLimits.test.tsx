import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { WorkspaceLimits } from './WorkspaceLimits'

it('reads limits, shows blocked Grants, and saves explicit empty lists', async () => {
  const api = {
    GET: vi.fn(async () => ({ data: { permitted_plugins: null, permitted_connections: ['account'], max_mail_visibility: 'escalations', outside_plugins: 2, outside_connections: 1 } })),
    PUT: vi.fn(async () => ({ data: {} })),
  }
  render(<QueryClientProvider client={new QueryClient()}><WorkspaceLimits api={api as unknown as ApiClient} workspaceId="team" name="Support" /></QueryClientProvider>)
  expect(await screen.findByText('2 Plugin Grants and 1 Connection Grants are outside the limits.')).toBeTruthy()
  fireEvent.click(screen.getByLabelText('Permit all Plugins for Support'))
  fireEvent.change(screen.getByLabelText('Permitted Connections for Support'), { target: { value: 'one, two' } })
  fireEvent.click(screen.getByRole('button', { name: 'Save limits for Support' }))
  await waitFor(() => expect(api.PUT).toHaveBeenCalledWith('/api/v1/administration/workspaces/{workspace_id}/limits', {
    params: { path: { workspace_id: 'team' } },
    body: { permitted_plugins: [], permitted_connections: ['one', 'two'], max_mail_visibility: 'escalations' },
  }))
})
