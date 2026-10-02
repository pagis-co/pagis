// The sign-in page: the password form, what it says on a server where
// nobody can sign in yet, and the Sign-In Link form that a browser on
// another machine gets in Remote Access.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, SignInMethod } from './api/client'
import { userKey } from './queries'
import { SignIn, WHERE_TO_GET_A_LINK, linkSecret } from './SignIn'

const SECRET = 'b'.repeat(64)
const PERSON = { id: 'user-2', email: 'grace@example.com' }

function mount(
  administrationOrigin: string | null,
  method: SignInMethod = 'password',
  api = { GET: vi.fn(), POST: vi.fn() },
) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={queryClient}>
      <SignIn
        api={api as unknown as ApiClient}
        administrationOrigin={administrationOrigin}
        method={method}
      />
    </QueryClientProvider>,
  )
  return { api, queryClient }
}

describe('SignIn', () => {
  it('names where the first Administrator is made while nobody can sign in', () => {
    mount('http://127.0.0.1:4701')

    expect(screen.getByText(/No Administrator exists yet/)).toBeTruthy()
    expect(screen.getByText('http://127.0.0.1:4701/')).toBeTruthy()
    expect(screen.getByText(/PAGIS_ADMIN_EMAIL/)).toBeTruthy()
    // The Administration Port binds loopback, so the page gives no link
    // that a browser on another machine would follow.
    expect(screen.queryByRole('link')).toBeNull()
  })

  it('heads the card with the mark and the name', () => {
    mount(null)

    const title = screen.getByRole('heading', { name: 'Pagis' })
    expect(title.querySelector('.ui-logo-mark')).not.toBeNull()
  })

  it('says nothing more once somebody can sign in', () => {
    mount(null)

    expect(screen.queryByText(/No Administrator exists yet/)).toBeNull()
    expect(screen.getByRole('button', { name: 'Sign in' })).toBeTruthy()
  })

  it('keeps the address and the password for a browser that signs in with a password', () => {
    mount(null, 'password')

    expect(screen.getByLabelText('Email')).toBeTruthy()
    expect(screen.getByLabelText('Password')).toBeTruthy()
    expect(screen.queryByLabelText('Paste a sign-in link')).toBeNull()
  })

  it('shows the daemon refusal of a password in Remote Access', async () => {
    const message =
      'This Pagis takes no password from another machine. Sign in with a Sign-In Link.'
    mount(null, 'password', {
      GET: vi.fn(),
      POST: vi.fn(async () => ({
        data: undefined,
        error: { error: { code: 'forbidden', message } },
        response: new Response(null, { status: 403 }),
      })),
    })

    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))

    expect((await screen.findByRole('alert')).textContent).toBe(message)
  })
})

describe('SignIn with a Sign-In Link', () => {
  it('shows one field for a link, and where to get one, with no password', () => {
    mount(null, 'link')

    expect(screen.getByLabelText('Paste a sign-in link')).toBeTruthy()
    expect(screen.getByText(WHERE_TO_GET_A_LINK)).toBeTruthy()
    expect(screen.queryByLabelText('Password')).toBeNull()
    expect(screen.queryByLabelText('Email')).toBeNull()
    expect(screen.getByRole('button', { name: 'Sign in' }).hasAttribute('disabled')).toBe(true)
  })

  it('trades the secret of a whole link and opens the app as the person', async () => {
    const { api, queryClient } = mount(null, 'link', {
      GET: vi.fn(),
      POST: vi.fn(async () => ({ data: PERSON, response: new Response(null) })),
    })

    fireEvent.change(screen.getByLabelText('Paste a sign-in link'), {
      target: { value: `  https://owner-mac.tail1234.ts.net/sign-in#${SECRET} ` },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/sessions/link', {
        body: expect.objectContaining({ secret: SECRET }),
      }),
    )
    await waitFor(() => expect(queryClient.getQueryData(userKey)).toEqual(PERSON))
  })

  it('trades a secret that is pasted alone', async () => {
    const { api } = mount(null, 'link', {
      GET: vi.fn(),
      POST: vi.fn(async () => ({ data: PERSON, response: new Response(null) })),
    })

    fireEvent.change(screen.getByLabelText('Paste a sign-in link'), {
      target: { value: SECRET },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/sessions/link', {
        body: expect.objectContaining({ secret: SECRET }),
      }),
    )
  })

  it('says why a link was refused', async () => {
    mount(null, 'link', {
      GET: vi.fn(),
      POST: vi.fn(async () => ({
        data: undefined,
        response: new Response(null, { status: 401 }),
      })),
    })

    fireEvent.change(screen.getByLabelText('Paste a sign-in link'), {
      target: { value: SECRET },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))

    expect((await screen.findByRole('alert')).textContent).toMatch(/spent or expired/)
  })

  it('reads the secret of a link or the secret alone', () => {
    expect(linkSecret(`https://pagis.example.net/sign-in#${SECRET}`)).toBe(SECRET)
    expect(linkSecret(` ${SECRET} `)).toBe(SECRET)
    expect(linkSecret('https://pagis.example.net/sign-in#a%20b')).toBe('a b')
    expect(linkSecret('https://pagis.example.net/sign-in#%E0%A4%A')).toBe('%E0%A4%A')
    expect(linkSecret('')).toBe('')
  })
})
