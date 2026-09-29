// The Vault section: one row per sign-in, Add a sign-in as the
// one primary action, Replace and Delete on each row, and the secret file
// hint. The password goes one way: nothing here reads it back.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { Vault } from './Vault'

const SAGE = { id: 'agent-sage', name: 'Sage', role: 'Chief of Staff' }

const CREDENTIALS = [
  {
    id: 'cred-1',
    domain: 'lufthansa.com',
    username: 'ada@example.com',
    login_url: 'https://lufthansa.com/login',
    provenance: 'user_supplied',
    has_totp: true,
    owner_agent_id: 'agent-sage',
    created_at: 1,
  },
  {
    id: 'cred-2',
    domain: 'northwind.co',
    username: 'ada',
    login_url: 'https://portal.northwind.co',
    provenance: 'user_supplied',
    has_totp: false,
    owner_agent_id: null,
    created_at: 2,
  },
]

function stubApi(credentials = CREDENTIALS) {
  return {
    GET: vi.fn(async (path: string) => {
      switch (path) {
        case '/api/v1/settings/credentials':
          return { data: { items: credentials } }
        case '/api/v1/agents':
          return { data: { items: [SAGE] } }
        default:
          throw new Error(`unexpected GET ${path}`)
      }
    }),
    POST: vi.fn(async () => ({ data: { ...CREDENTIALS[1], id: 'cred-3' } })),
    DELETE: vi.fn(async () => ({ error: undefined, response: { ok: true } })),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={queryClient}>
      <Vault api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

function fillSignIn(fields: Record<string, string>) {
  for (const [label, value] of Object.entries(fields)) {
    fireEvent.change(screen.getByLabelText(label), { target: { value } })
  }
}

describe('Vault', () => {
  it('lists one row per sign-in with its login address, 2FA and owner', async () => {
    mount(stubApi())

    const lufthansa = (await screen.findByText('lufthansa.com')).closest(
      '.ui-row',
    ) as HTMLElement
    expect(lufthansa.textContent).toContain('login at lufthansa.com/login')
    expect(lufthansa.textContent).toContain('2FA')
    expect(lufthansa.textContent).toContain('Owner')
    await waitFor(() => expect(lufthansa.textContent).toContain('Sage'))

    const northwind = screen.getByText('northwind.co').closest('.ui-row') as HTMLElement
    expect(northwind.textContent).toContain('login at portal.northwind.co')
    expect(northwind.textContent).not.toContain('2FA')
    expect(northwind.textContent).toContain('Any sprite with a Grant')

    expect(screen.getByRole('heading', { level: 3, name: 'Vault' })).toBeTruthy()
    expect(screen.getByText(/stays in the installation's sealed secret file/)).toBeTruthy()
    expect(screen.getAllByRole('button', { name: /^Replace/ })).toHaveLength(2)
  })

  it('says so when the vault is empty', async () => {
    mount(stubApi([]))
    expect(await screen.findByText('No sign-ins yet.')).toBeTruthy()
  })

  it('adds a sign-in from the one primary action', async () => {
    const api = stubApi()
    mount(api)

    await userEvent.click(await screen.findByRole('button', { name: 'Add a sign-in' }))
    fillSignIn({
      Site: 'example.com',
      'Username or email': 'alice@example.com',
      'Sign-in address': 'https://example.com/login',
      Password: 'hunter2',
    })
    fireEvent.click(screen.getByRole('button', { name: 'Save the sign-in' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/credentials', {
        body: {
          domain: 'example.com',
          username: 'alice@example.com',
          login_url: 'https://example.com/login',
          secret: 'hunter2',
          totp_seed: undefined,
        },
      }),
    )
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(api.DELETE).not.toHaveBeenCalled()
  })

  it('replaces a sign-in: the new one is saved before the old one goes', async () => {
    const api = stubApi()
    mount(api)

    await userEvent.click(
      await screen.findByRole('button', { name: 'Replace the sign-in for northwind.co' }),
    )
    expect((screen.getByLabelText('Site') as HTMLInputElement).value).toBe('northwind.co')
    expect((screen.getByLabelText('Username or email') as HTMLInputElement).value).toBe('ada')
    expect((screen.getByLabelText('Password') as HTMLInputElement).value).toBe('')
    fillSignIn({ Password: 'new-secret', 'One-time code seed': 'JBSWY3DP' })
    fireEvent.click(screen.getByRole('button', { name: 'Save the sign-in' }))

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith(
        '/api/v1/settings/credentials/{credential_id}',
        { params: { path: { credential_id: 'cred-2' } } },
      ),
    )
    expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/credentials', {
      body: {
        domain: 'northwind.co',
        username: 'ada',
        login_url: 'https://portal.northwind.co',
        secret: 'new-secret',
        totp_seed: 'JBSWY3DP',
      },
    })
    expect(api.POST.mock.invocationCallOrder[0]).toBeLessThan(
      api.DELETE.mock.invocationCallOrder[0],
    )
  })

  it('keeps the old sign-in when the new one is refused', async () => {
    const api = stubApi()
    api.POST.mockResolvedValueOnce({
      error: { message: 'The address is not on that site.' },
    } as never)
    mount(api)

    await userEvent.click(
      await screen.findByRole('button', { name: 'Replace the sign-in for northwind.co' }),
    )
    fillSignIn({ Password: 'new-secret' })
    fireEvent.click(screen.getByRole('button', { name: 'Save the sign-in' }))

    expect(await screen.findByRole('alert')).toBeTruthy()
    expect(api.DELETE).not.toHaveBeenCalled()
    expect(screen.getByRole('dialog')).toBeTruthy()
  })

  it('deletes a sign-in', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(
      await screen.findByRole('button', { name: 'Delete the sign-in for lufthansa.com' }),
    )
    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith(
        '/api/v1/settings/credentials/{credential_id}',
        { params: { path: { credential_id: 'cred-1' } } },
      ),
    )
  })
})
