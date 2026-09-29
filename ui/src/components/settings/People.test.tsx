// The People section: the roster, the account the administrator
// creates, the cap, the reset and the disable switch.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { People, money, personLabel } from './People'

const ADMINISTRATOR = {
  id: 'u-admin',
  email: null,
  name: 'Ada',
  role: 'administrator',
  workspace_id: 'ws-1',
  disabled: false,
  last_signed_in_at: 1_700_000_000_000,
  monthly_spend_cap_usd: null,
  created_at: 1,
}

const MEMBER = {
  id: 'u-grace',
  email: 'grace@example.com',
  name: 'Grace',
  role: 'member',
  workspace_id: 'ws-2',
  disabled: false,
  last_signed_in_at: null,
  monthly_spend_cap_usd: 25,
  created_at: 2,
}

function total(costUsd: number) {
  return {
    input_tokens: 10,
    output_tokens: 5,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    cost_usd: costUsd,
    calls: 1,
  }
}

function stubApi() {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/administration/people') {
        return { data: { items: [ADMINISTRATOR, MEMBER] } }
      }
      if (path === '/api/v1/administration/usage') {
        return {
          data: {
            from: 0,
            to: 1,
            total: total(30),
            items: [
              { person: MEMBER, total: total(26), cap_reached: true },
              { person: ADMINISTRATOR, total: total(4), cap_reached: false },
            ],
          },
        }
      }
      throw new Error(`unexpected GET ${path}`)
    }),
    POST: vi.fn(async () => ({ data: MEMBER })),
    PUT: vi.fn(async () => ({ data: MEMBER })),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={queryClient}>
      <People api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

describe('People', () => {
  it('names every person with their role, their spend and their cap', async () => {
    mount(stubApi())

    expect(await screen.findByText('Ada')).toBeTruthy()
    const grace = (await screen.findByText('Grace')).closest('.people-row') as HTMLElement
    expect(within(grace).getByText('grace@example.com')).toBeTruthy()
    expect(within(grace).getByText('$26.00 this month')).toBeTruthy()
    // The roster says who is stopped.
    expect(within(grace).getByText('At their cap')).toBeTruthy()
    expect(
      (within(grace).getByLabelText('Monthly spend cap for Grace') as HTMLInputElement).value,
    ).toBe('25')

    const ada = (screen.getByText('Ada').closest('.people-row')) as HTMLElement
    expect(within(ada).getByText('Administrator')).toBeTruthy()
    expect(await screen.findByText(/\$30.00 spent on model calls this month/)).toBeTruthy()
  })

  it('creates an account from the address, the name and a first password', async () => {
    const api = stubApi()
    mount(api)
    await screen.findByText('Grace')

    await userEvent.type(screen.getByLabelText('Email address'), 'mabel@example.com')
    await userEvent.type(screen.getByLabelText('Name'), 'Mabel')
    const create = screen.getByRole('button', { name: 'Create account' })
    // A short password is not an account.
    await userEvent.type(screen.getByLabelText('First password'), 'short')
    expect(create.getAttribute('disabled')).not.toBeNull()

    await userEvent.clear(screen.getByLabelText('First password'))
    await userEvent.type(screen.getByLabelText('First password'), 'correct horse battery')
    await userEvent.click(create)

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/administration/people', {
        body: {
          email: 'mabel@example.com',
          name: 'Mabel',
          password: 'correct horse battery',
        },
      }),
    )
  })

  it('disables an account and offers to enable a disabled one', async () => {
    const api = stubApi()
    api.GET = vi.fn(async (path: string) => {
      if (path === '/api/v1/administration/people') {
        return { data: { items: [ADMINISTRATOR, { ...MEMBER, disabled: true }] } }
      }
      return { data: { from: 0, to: 1, total: total(0), items: [] } }
    })
    mount(api)

    const grace = (await screen.findByText('Grace')).closest('.people-row') as HTMLElement
    expect(within(grace).getByText('Disabled')).toBeTruthy()
    await userEvent.click(
      within(grace).getByRole('button', { name: 'Enable the account of Grace' }),
    )

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/administration/people/{user_id}/enable',
        { params: { path: { user_id: 'u-grace' } } },
      ),
    )
  })

  it('saves a cap, clears one, and refuses a cap that is not an amount', async () => {
    const api = stubApi()
    mount(api)
    const grace = (await screen.findByText('Grace')).closest('.people-row') as HTMLElement
    const cap = within(grace).getByLabelText('Monthly spend cap for Grace')
    const save = within(grace).getByRole('button', { name: 'Save the spend cap for Grace' })

    await userEvent.clear(cap)
    await userEvent.type(cap, '-5')
    expect(save.getAttribute('disabled')).not.toBeNull()

    await userEvent.clear(cap)
    await userEvent.type(cap, '40')
    await userEvent.click(save)
    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith(
        '/api/v1/administration/people/{user_id}/spend-cap',
        {
          params: { path: { user_id: 'u-grace' } },
          body: { monthly_spend_cap_usd: 40 },
        },
      ),
    )

    // An empty field is no cap at all.
    await userEvent.clear(cap)
    await userEvent.click(save)
    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith(
        '/api/v1/administration/people/{user_id}/spend-cap',
        {
          params: { path: { user_id: 'u-grace' } },
          body: { monthly_spend_cap_usd: null },
        },
      ),
    )
  })

  it('resets a password only once it is long enough', async () => {
    const api = stubApi()
    mount(api)
    const grace = (await screen.findByText('Grace')).closest('.people-row') as HTMLElement
    const field = within(grace).getByLabelText('New password for Grace')
    const reset = within(grace).getByRole('button', {
      name: 'Reset the password of Grace',
    })

    await userEvent.type(field, 'short')
    expect(reset.getAttribute('disabled')).not.toBeNull()

    await userEvent.clear(field)
    await userEvent.type(field, 'another long password')
    await userEvent.click(reset)

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/administration/people/{user_id}/password',
        {
          params: { path: { user_id: 'u-grace' } },
          body: { password: 'another long password' },
        },
      ),
    )
  })
})

/** The seeded person of a local installation holds no address and no
 *  password: the client trades the Client Credential. The roster
 *  is where they set a way in for a browser, and a person who
 *  already has one is not asked again. */
describe('a person with no way in', () => {
  it('sets an address and a password for them', async () => {
    const api = stubApi()
    mount(api)

    const ada = (await screen.findByText('Ada')).closest('.people-row') as HTMLElement
    const set = within(ada).getByRole('button', { name: 'Set a way in for Ada' })
    expect(set.getAttribute('disabled')).not.toBeNull()

    await userEvent.type(within(ada).getByLabelText('Email address for Ada'), 'ada@example.com')
    await userEvent.type(
      within(ada).getByLabelText('First password for Ada'),
      'correct horse battery',
    )
    await userEvent.click(set)

    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith(
        '/api/v1/administration/people/{user_id}/sign-in',
        {
          params: { path: { user_id: 'u-admin' } },
          body: { email: 'ada@example.com', password: 'correct horse battery' },
        },
      ),
    )
  })

  it('does not ask a person who signs in with an address already', async () => {
    mount(stubApi())

    const grace = (await screen.findByText('Grace')).closest('.people-row') as HTMLElement
    expect(within(grace).queryByLabelText('Email address for Grace')).toBeNull()
  })
})

describe('the roster labels', () => {
  it('names a person by their name, then their address', () => {
    expect(personLabel({ ...MEMBER, name: 'Grace' })).toBe('Grace')
    expect(personLabel({ ...MEMBER, name: null })).toBe('grace@example.com')
    expect(personLabel({ ...MEMBER, name: null, email: null })).toBe('This person')
  })

  it('writes money to the cent', () => {
    expect(money(0)).toBe('$0.00')
    expect(money(1.5)).toBe('$1.50')
    expect(money(12.345)).toBe('$12.35')
  })
})
