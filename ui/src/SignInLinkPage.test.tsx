// The page a Sign-In Link opens: it posts the secret of the fragment
// once, takes it out of the address bar, and says why a refused link
// signs nobody in.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from './api/client'
import { SignInLinkPage } from './SignInLinkPage'

function mount(post: ReturnType<typeof vi.fn>, onSignedIn = vi.fn()) {
  const api = { GET: vi.fn(), POST: post }
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={queryClient}>
      <SignInLinkPage api={api as unknown as ApiClient} onSignedIn={onSignedIn} />
    </QueryClientProvider>,
  )
  return onSignedIn
}

function open(address: string) {
  window.history.replaceState(null, '', address)
}

afterEach(() => open('/'))

describe('SignInLinkPage', () => {
  it('posts the secret, clears it from the address and opens the app', async () => {
    open('/sign-in#s3cret-value')
    const post = vi.fn(async () => ({
      data: { user: { id: 'u-1' } },
      response: new Response(null, { status: 200 }),
    }))

    const onSignedIn = mount(post)

    await waitFor(() => expect(onSignedIn).toHaveBeenCalledTimes(1))
    expect(post).toHaveBeenCalledTimes(1)
    expect(post).toHaveBeenCalledWith('/api/v1/sessions/link', {
      body: { secret: 's3cret-value', timezone: expect.any(String) },
    })
    expect(window.location.pathname).toBe('/sign-in')
    expect(window.location.hash).toBe('')
  })

  it('says a spent or expired link is spent, and offers the password sign-in', async () => {
    open('/sign-in#spent')
    const post = vi.fn(async () => ({
      data: undefined,
      response: new Response(null, { status: 401 }),
    }))

    const onSignedIn = mount(post)

    expect((await screen.findByRole('alert')).textContent).toBe(
      'This sign-in link is spent or expired. Ask for a new link.',
    )
    expect(screen.getByRole('link', { name: /Sign in with an address/ })).toBeTruthy()
    expect(onSignedIn).not.toHaveBeenCalled()
  })

  it('says to wait after too many refused links', async () => {
    open('/sign-in#guess')
    const post = vi.fn(async () => ({
      data: undefined,
      response: new Response(null, { status: 429 }),
    }))

    mount(post)

    expect((await screen.findByRole('alert')).textContent).toMatch(/Too many attempts/)
  })

  it('posts nothing when the address holds no secret', async () => {
    open('/sign-in')
    const post = vi.fn()

    mount(post)

    expect((await screen.findByRole('alert')).textContent).toMatch(/holds no sign-in link/)
    expect(post).not.toHaveBeenCalled()
  })
})
