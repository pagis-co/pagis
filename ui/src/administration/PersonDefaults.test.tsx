import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import type { ApiClient } from '../api/client'
import { PersonDefaults } from './PersonDefaults'

it('saves an address rule and a default Shared Workspace with its role', async () => {
  const api = {
    GET: vi.fn(async (path: string) => path.endsWith('person-defaults') ? { data: {
      defaults: { address_rule: null, mailbox_connection_id: null, shared_workspaces: [] },
      mailbox_offers: [{ connection_id: 'mail', domain: 'example.com' }],
    } } : { data: { items: [{ id: 'team', name: 'Support', kind: 'shared' }, { id: 'private', name: 'Private', kind: 'private' }] } }),
    PUT: vi.fn(async () => ({ response: new Response(null, { status: 204 }) })),
  }
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <PersonDefaults api={api as unknown as ApiClient} />
  </QueryClientProvider>)
  fireEvent.click(await screen.findByLabelText('Address rule'))
  fireEvent.click(screen.getByRole('option', { name: 'First name.last name' }))
  fireEvent.click(screen.getByLabelText('Mailbox Provider'))
  fireEvent.click(screen.getByRole('option', { name: 'example.com' }))
  fireEvent.click(screen.getByLabelText('Join Support'))
  expect(screen.getByLabelText('Role in Support').textContent).toContain('Responder')
  fireEvent.click(screen.getByLabelText('Role in Support'))
  fireEvent.click(screen.getByRole('option', { name: 'Reader' }))
  expect(screen.queryByLabelText('Join Private')).toBeNull()
  fireEvent.click(screen.getByRole('button', { name: 'Save defaults' }))
  await waitFor(() => expect(api.PUT).toHaveBeenCalledWith('/api/v1/administration/person-defaults', {
    body: { address_rule: 'first_last', mailbox_connection_id: 'mail', shared_workspaces: [{ workspace_id: 'team', role: 'reader' }] },
  }))
  expect(await screen.findByText('Defaults saved.')).toBeTruthy()
})
