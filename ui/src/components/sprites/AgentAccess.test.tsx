import { QueryClient } from '@tanstack/react-query'
import { QueryClientProvider } from '../../test/QueryClientProvider'
import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { expect, it, vi } from 'vitest'
import type { AgentDto, ApiClient } from '../../api/client'
import { AgentAccess } from './AgentAccess'

it('grants and revokes outbound Calls from the Workspace Number', async () => {
  const grants: unknown[] = []
  const api = {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/workspace/number') return { data: { number: { id: 'num1', e164: '+14155550123', status: 'active' }, carrier: null } }
      return { data: { items: path === '/api/v1/grants' ? [...grants] : [] } }
    }),
    POST: vi.fn(async () => {
      const grant = { id: 'g1', agent_id: 'ag1', resource_kind: 'workspace_number', resource_id: 'num1' }
      grants.push(grant)
      return { data: grant }
    }),
    DELETE: vi.fn(async () => {
      grants.length = 0
      return { response: { status: 204 } }
    }),
  }
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <AgentAccess api={api as unknown as ApiClient} agent={{ id: 'ag1', name: 'Sage' } as AgentDto} />
    </QueryClientProvider>,
  )
  const toggle = await screen.findByRole('switch', { name: 'Call from the Workspace Number' })
  expect(toggle.getAttribute('aria-checked')).toBe('false')
  await userEvent.click(toggle)
  await waitFor(() => expect(api.POST).toHaveBeenCalledWith('/api/v1/grants/workspace-number', { body: { agent_id: 'ag1', number_id: 'num1' } }))
  await waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('true'))
  await userEvent.click(toggle)
  await waitFor(() => expect(api.DELETE).toHaveBeenCalled())
  await waitFor(() => expect(toggle.getAttribute('aria-checked')).toBe('false'))
})

it('shows no Workspace Number switch when the Workspace has no number', async () => {
  const api = {
    GET: vi.fn(async (path: string) => ({
      data: path === '/api/v1/workspace/number' ? { number: null, carrier: null } : { items: [] },
    })),
  }
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <AgentAccess api={api as unknown as ApiClient} agent={{ id: 'ag1', name: 'Sage' } as AgentDto} />
    </QueryClientProvider>,
  )
  await waitFor(() => expect(api.GET).toHaveBeenCalledWith('/api/v1/workspace/number'))
  expect(screen.queryByRole('switch', { name: 'Call from the Workspace Number' })).toBeNull()
})
