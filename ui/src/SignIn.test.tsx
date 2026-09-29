// The sign-in page, and what it says on a server where nobody can sign
// in yet.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from './api/client'
import { SignIn } from './SignIn'

function mount(administrationOrigin: string | null) {
  const api = { GET: vi.fn(), POST: vi.fn() }
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={queryClient}>
      <SignIn api={api as unknown as ApiClient} administrationOrigin={administrationOrigin} />
    </QueryClientProvider>,
  )
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
})
