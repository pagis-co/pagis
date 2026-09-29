// Agent host and Vault rule rows (ADR-0022).

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, GrantDto } from '../api/client'
import { GrantRow } from './GrantsSettings'

function stubApi() {
  return {
    PUT: vi.fn(async () => ({ data: {} })),
    DELETE: vi.fn(async () => ({
      error: undefined,
      response: { ok: true },
    })),
  }
}

const grant = {
  id: 'g1',
  agent_id: 'ag1',
  agent_name: 'Sage',
  resource_kind: 'host',
  resource_id: null,
  allow: ['git status', 'echo'],
  capabilities: [],
  revision: 1,
  created_at: 1,
}

function mount(api: ReturnType<typeof stubApi>, items: unknown[]) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      {items.map((item) => (
        <GrantRow
          key={(item as GrantDto).id}
          api={api as unknown as ApiClient}
          grant={item as GrantDto}
        />
      ))}
    </QueryClientProvider>,
  )
}

const credentialGrant = {
  id: 'g2',
  agent_id: 'ag1',
  agent_name: 'Sage',
  resource_kind: 'credential',
  resource_id: null,
  allow: ['example.com'],
  capabilities: [],
  revision: 1,
  created_at: 2,
}

describe('GrantRow', () => {
  it('lists a credential grant as vault domains', async () => {
    const api = stubApi()
    mount(api, [credentialGrant])

    expect(await screen.findByText('Vault domains')).toBeTruthy()
    expect(screen.getByText('example.com')).toBeTruthy()
    expect(screen.getByPlaceholderText('Add a domain, e.g. example.com')).toBeTruthy()
    expect(screen.getByText('Add domain')).toBeTruthy()
  })

  it('a credential grant with no domains asks every time', async () => {
    const items = [{ ...credentialGrant, allow: [] }]
    mount(stubApi(), items)

    expect(
      await screen.findByText(
        'No allowed domains — every saved login asks for an approval.',
      ),
    ).toBeTruthy()
  })

  it('lists live grants with agent, resource, and rules', async () => {
    const api = stubApi()
    mount(api, [grant])

    expect(await screen.findByText('Sage')).toBeTruthy()
    expect(screen.getByText('Host access')).toBeTruthy()
    expect(screen.getByText('git status')).toBeTruthy()
    expect(screen.getByText('echo')).toBeTruthy()
  })

  it('removing a rule replaces the rule list', async () => {
    const api = stubApi()
    mount(api, [grant])

    fireEvent.click(await screen.findByLabelText('Remove rule git status'))

    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/grants/{grant_id}/rules', {
        params: { path: { grant_id: 'g1' } },
        body: { allow: ['echo'] },
      }),
    )
  })

  it('adding a rule appends it to the rule list', async () => {
    const api = stubApi()
    mount(api, [grant])

    fireEvent.change(
      await screen.findByPlaceholderText(
        'Add a command prefix, e.g. git status',
      ),
      { target: { value: ' npm run ' } },
    )
    fireEvent.click(screen.getByText('Add rule'))

    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/grants/{grant_id}/rules', {
        params: { path: { grant_id: 'g1' } },
        body: { allow: ['git status', 'echo', 'npm run'] },
      }),
    )
  })

  it('revoke calls the delete endpoint', async () => {
    const api = stubApi()
    mount(api, [grant])

    fireEvent.click(await screen.findByText('Revoke'))

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/grants/{grant_id}', {
        params: { path: { grant_id: 'g1' } },
      }),
    )
  })
})
