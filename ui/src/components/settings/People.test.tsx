// The People section: the roster, the account the administrator
// creates, the cap, the reset and the disable switch.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
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
  created_at: 2,
}

/** The person the administrator creates. */
const MABEL = {
  ...MEMBER,
  id: 'u-mabel',
  email: 'mabel@example.com',
  name: 'Mabel',
  workspace_id: 'ws-3',
}

/** An invite as the daemon answers it: good for seven days. */
const INVITE = {
  url: 'https://pagis.example/sign-in#secret',
  expires_at: Date.now() + 7 * 24 * 60 * 60 * 1_000,
  qr_svg: '<svg xmlns="http://www.w3.org/2000/svg"></svg>',
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
              { workspace: { person: MEMBER }, total: total(26), cap_reached: true },
              { workspace: { person: ADMINISTRATOR }, total: total(4), cap_reached: false },
            ],
          },
        }
      }
      throw new Error(`unexpected GET ${path}`)
    }),
    POST: vi.fn(async (path: string) => {
      if (path === '/api/v1/administration/people') {
        return { data: { person: MABEL, invite: INVITE } }
      }
      if (path === '/api/v1/administration/people/{user_id}/sign-in-links') {
        return { data: INVITE }
      }
      return { data: MEMBER }
    }),
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
  it('names every Person with their role and spend, with no cap control', async () => {
    mount(stubApi())

    expect(await screen.findByText('Ada')).toBeTruthy()
    const grace = (await screen.findByText('Grace')).closest('.people-row') as HTMLElement
    expect(within(grace).getByText('grace@example.com')).toBeTruthy()
    expect(within(grace).queryByLabelText('Monthly spend cap for Grace')).toBeNull()

    const ada = (screen.getByText('Ada').closest('.people-row')) as HTMLElement
    expect(within(ada).getByText('Administrator')).toBeTruthy()
    expect(await screen.findByText(/\$30.00 spent on model calls this month/)).toBeTruthy()
  })

  it('changes a Person role and reads the saved roster', async () => {
    const api = stubApi()
    let role = 'member'
    api.GET = vi.fn(async (path: string) => path === '/api/v1/administration/people'
      ? { data: { items: [ADMINISTRATOR, { ...MEMBER, role }] } }
      : { data: { from: 0, to: 1, total: total(0), items: [] } })
    api.PUT = vi.fn(async () => {
      role = 'administrator'
      return { data: { ...MEMBER, role } }
    })
    mount(api)
    const control = await screen.findByRole('combobox', { name: 'Role for Grace' })
    expect(control.textContent).toContain('Member')
    fireEvent.click(control)
    fireEvent.click(await screen.findByRole('option', { name: 'Administrator' }))
    await waitFor(() => expect(api.PUT).toHaveBeenCalledWith(
      '/api/v1/administration/people/{user_id}/role',
      { params: { path: { user_id: 'u-grace' } }, body: { role: 'administrator' } },
    ))
    await waitFor(() => expect(control.textContent).toContain('Administrator'))
    expect(screen.getByRole('combobox', { name: 'Role for Ada' })).toBeTruthy()
  })

  it('shows a refused role change and keeps the saved role', async () => {
    const api = stubApi()
    api.PUT = vi.fn(async () => ({ error: { error: {
      code: 'conflict', message: 'The Org must have at least one enabled Administrator.',
    } } })) as unknown as typeof api.PUT
    mount(api)
    const control = await screen.findByRole('combobox', { name: 'Role for Ada' })
    fireEvent.click(control)
    fireEvent.click(await screen.findByRole('option', { name: 'Member' }))
    expect((await screen.findByRole('alert')).textContent).toContain('enabled Administrator')
    expect(control.textContent).toContain('Administrator')
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

  it('creates an account with no password and shows the invite', async () => {
    const api = stubApi()
    mount(api)
    await screen.findByText('Grace')

    await userEvent.type(screen.getByLabelText('Email address'), 'mabel@example.com')
    await userEvent.type(screen.getByLabelText('Name'), 'Mabel')
    await userEvent.click(screen.getByRole('button', { name: 'Create account' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/administration/people', {
        body: { email: 'mabel@example.com', name: 'Mabel' },
      }),
    )
    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByText(/Send this link to Mabel/)).toBeTruthy()
    expect(
      (within(dialog).getByLabelText('Sign-in link') as HTMLInputElement).value,
    ).toBe(INVITE.url)
    expect(within(dialog).getByAltText('QR code of the sign-in link')).toBeTruthy()
    expect(within(dialog).getByText(/Expires in 7 days/)).toBeTruthy()
  })

  it('makes a new invite for a person on the roster', async () => {
    const api = stubApi()
    mount(api)
    const grace = (await screen.findByText('Grace')).closest('.people-row') as HTMLElement

    await userEvent.click(within(grace).getByRole('button', { name: 'New invite for Grace' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/administration/people/{user_id}/sign-in-links',
        { params: { path: { user_id: 'u-grace' } } },
      ),
    )
    const dialog = await screen.findByRole('dialog')
    expect(
      (within(dialog).getByLabelText('Sign-in link') as HTMLInputElement).value,
    ).toBe(INVITE.url)
  })

  it('offers no invite for a disabled account', async () => {
    const api = stubApi()
    api.GET = vi.fn(async (path: string) => {
      if (path === '/api/v1/administration/people') {
        return { data: { items: [ADMINISTRATOR, { ...MEMBER, disabled: true }] } }
      }
      return { data: { from: 0, to: 1, total: total(0), items: [] } }
    })
    mount(api)

    const grace = (await screen.findByText('Grace')).closest('.people-row') as HTMLElement
    const invite = within(grace).getByRole('button', { name: 'New invite for Grace' })
    expect(invite.getAttribute('disabled')).not.toBeNull()
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

// At a phone width, one line of a roster row holds more than the width:
// the name, the email, the role, the spend and two buttons. The line
// wraps, so no control runs past the edge or covers another.
describe('a roster row on a narrow screen', () => {
  it('lets each line of the row wrap', async () => {
    mount(stubApi())

    const grace = (await screen.findByText('Grace')).closest('.people-row') as HTMLElement
    const lines = [...grace.children].filter((child) => child.classList.contains('ui-row'))
    expect(lines.length).toBe(2)
    for (const line of lines) expect(line.classList.contains('people-line')).toBe(true)

    const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'People.css'), 'utf8')
    expect(css).toMatch(/\.people-line\s*\{[^}]*flex-wrap:\s*wrap/)
  })
})


it('shows the taken address and sends the explicit address on retry', async () => {
  const api = stubApi()
  api.POST.mockResolvedValueOnce({ error: { error: { code: 'conflict', message: 'ada@example.com is taken. Try ada2@example.com.' } } } as never)
  mount(api)
  fireEvent.change(screen.getByLabelText('Email address'), { target: { value: 'ada@example.com' } })
  fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'Ada Lovelace' } })
  fireEvent.click(screen.getByRole('button', { name: 'Create account' }))
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', 'ada@example.com is taken. Try ada2@example.com.')
  fireEvent.change(screen.getByLabelText('Workspace Mailbox address'), { target: { value: 'ada2@example.com' } })
  fireEvent.click(screen.getByRole('button', { name: 'Create account' }))
  await waitFor(() => expect(api.POST).toHaveBeenLastCalledWith('/api/v1/administration/people', {
    body: { email: 'ada@example.com', name: 'Ada Lovelace', mailbox_address: 'ada2@example.com' },
  }))
})
