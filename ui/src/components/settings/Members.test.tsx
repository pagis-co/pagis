import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import type { ApiClient } from '../../api/client'
import { CurrentWorkspaceContext, WorkspaceSelectionContext } from '../../workspace'
import { Members } from './Members'

function mount(role: 'manager' | 'reader') {
  const api = {
    GET: vi.fn(async (path: string) => path === '/api/v1/colleagues' ? { data: { items: [{ person_id: 'next', name: 'Quinn', workspace_id: 'private-next', presence: 'away' }] } } : ({ data: { items: [{ person_id: 'person', name: 'Sam', email: 'sam@example.com', role: 'manager' }] } })),
    POST: vi.fn(async () => ({})),
    PATCH: vi.fn(async () => ({})),
    DELETE: vi.fn(async () => ({})),
  }
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <CurrentWorkspaceContext value="shared">
        <WorkspaceSelectionContext value={{ items: [], current: { id: 'shared', name: 'Support', kind: 'shared', role }, select: () => {} }}>
          <Members api={api as unknown as ApiClient} />
        </WorkspaceSelectionContext>
      </CurrentWorkspaceContext>
    </QueryClientProvider>,
  )
  return api
}

describe('Shared Workspace Members', () => {
  it('lets a Manager add, change and remove Persons', async () => {
    const api = mount('manager')
    await screen.findByText('Sam')
    fireEvent.click(screen.getByRole('combobox', { name: 'Person' }))
    fireEvent.click(await screen.findByRole('option', { name: 'Quinn' }))
    fireEvent.click(screen.getByRole('button', { name: 'Add Person' }))
    await waitFor(() => expect(api.POST).toHaveBeenCalled())
    fireEvent.click(screen.getByRole('combobox', { name: 'Role for Sam' }))
    fireEvent.click(await screen.findByRole('option', { name: 'Reader' }))
    await waitFor(() => expect(api.PATCH).toHaveBeenCalled())
    fireEvent.click(screen.getByRole('button', { name: 'Remove Sam' }))
    await waitFor(() => expect(api.DELETE).toHaveBeenCalled())
  })

  it('gives a Reader no membership controls', () => {
    mount('reader')
    expect(screen.queryByRole('button', { name: 'Add Person' })).toBeNull()
  })
})
