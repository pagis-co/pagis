// The session: the app asks the daemon who is signed in. With
// no session the sign-in page takes the screen; a sign-in shows the
// app without a reload; a sign-out gives the sign-in page back. The
// session lives in an HTTP-only cookie, so the browser stores nothing.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory } from '@tanstack/react-router'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from './api/client'
import { App } from './App'
import { openDocument } from './navigation'
import { shellResponse } from './test/appStub'

// A document load leaves the page, which jsdom cannot do. The test
// reads the address the page opens instead.
vi.mock('./navigation', () => ({ openDocument: vi.fn() }))

const { api } = vi.hoisted(() => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
}))

vi.mock('./api/client', async () => {
  const actual = await vi.importActual<typeof import('./api/client')>('./api/client')
  return { ...actual, createApiClient: () => api as unknown as ApiClient }
})

const { sockets } = vi.hoisted(() => ({
  sockets: [] as { handlers: { onSignedOut: () => void }; stopped: boolean }[],
}))

vi.mock('./ws/socket', () => ({
  PagisSocket: class {
    handlers: { onSignedOut: () => void }
    stopped = false
    constructor(options: { handlers: { onSignedOut: () => void } }) {
      this.handlers = options.handlers
      sockets.push(this)
    }
    start() {}
    stop() {
      this.stopped = true
    }
    subscribeChannel() {}
  },
}))

const person = { id: 'user-1', name: 'Ada', email: 'ada@example.com', role: 'member' }

/** The answer of a route that refuses the request. */
function refused(status: number, message: string) {
  return { error: { error: { code: 'x', message } }, response: { status } }
}

function mount(url = '/') {
  const history = createMemoryHistory({ initialEntries: [url] })
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <App history={history} />
    </QueryClientProvider>,
  )
  return history
}

beforeEach(() => {
  sockets.length = 0
  window.localStorage.clear()
  vi.mocked(openDocument).mockReset()
  api.GET.mockReset()
  api.POST.mockReset()
  api.DELETE.mockReset()
  // No session yet: every read of the person is refused.
  api.GET.mockImplementation(async (path: string) =>
    path === '/api/v1/user'
      ? refused(401, 'sign in first')
      : shellResponse(path),
  )
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response('missing', { status: 404 })),
  )
})

describe('sign-in', () => {
  it('shows the sign-in page when no session exists', async () => {
    mount()

    expect(await screen.findByRole('button', { name: 'Sign in' })).toBeTruthy()
  })

  it('sends the address and the password, then shows the app', async () => {
    api.POST.mockImplementation(async () => ({
      data: person,
      response: { status: 200 },
    }))
    mount()

    fireEvent.change(await screen.findByLabelText('Email'), {
      target: { value: 'ada@example.com' },
    })
    fireEvent.change(screen.getByLabelText('Password'), {
      target: { value: 'secret' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Sign in' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/sessions', {
        // The device's timezone goes with it: the first sign-in makes
        // it the Person's own.
        body: {
          email: 'ada@example.com',
          password: 'secret',
          timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
        },
      }),
    )
    expect(await screen.findByRole('navigation', { name: 'Places' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Sign in' })).toBeNull()
    // The cookie is the session: the browser keeps no copy of it.
    expect(window.localStorage.length).toBe(0)
  })

  // A link opened with no session keeps its address: the sign-in opens
  // the conversation it names.
  it('opens the address the page came from after the sign-in', async () => {
    api.POST.mockImplementation(async () => ({
      data: person,
      response: { status: 200 },
    }))
    const history = mount('/c/channel-2')

    fireEvent.click(await screen.findByRole('button', { name: 'Sign in' }))

    expect(await screen.findByRole('heading', { name: 'Launch planning' })).toBeTruthy()
    expect(history.location.pathname).toBe('/c/channel-2')
  })

  // The start route of a Google authorization sends a browser with no
  // session here. The sign-in keeps the address, and the address sends
  // the browser back to the start route of the daemon.
  it('goes back to the Google start route after the sign-in', async () => {
    api.POST.mockImplementation(async () => ({
      data: person,
      response: { status: 200 },
    }))
    const history = mount('/connections/google/start?state=Zx9-abc_123')

    fireEvent.click(await screen.findByRole('button', { name: 'Sign in' }))
    expect(openDocument).not.toHaveBeenCalled()

    await waitFor(() =>
      expect(openDocument).toHaveBeenCalledWith(
        '/api/v1/connections/google/start?state=Zx9-abc_123',
      ),
    )
    expect(history.location.pathname).toBe('/connections/google/start')
  })

  it('reports an address and a password that do not match', async () => {
    api.POST.mockImplementation(async () => refused(401, 'no'))
    mount()

    fireEvent.click(await screen.findByRole('button', { name: 'Sign in' }))

    expect(
      await screen.findByText('The address and the password do not match.'),
    ).toBeTruthy()
  })

  it('reports too many attempts apart from a wrong password', async () => {
    api.POST.mockImplementation(async () => refused(429, 'slow down'))
    mount()

    fireEvent.click(await screen.findByRole('button', { name: 'Sign in' }))

    expect(
      await screen.findByText('Too many attempts. Wait a minute, then sign in again.'),
    ).toBeTruthy()
  })
})

describe('sign-out', () => {
  it('ends the session and gives the sign-in page back', async () => {
    // The daemon clears the cookie, so the next read of the person is
    // refused.
    let signedIn = true
    api.GET.mockImplementation(async (path: string) => {
      if (path !== '/api/v1/user') return shellResponse(path)
      return signedIn
        ? { data: person, response: { status: 200 } }
        : refused(401, 'sign in first')
    })
    api.DELETE.mockImplementation(async () => {
      signedIn = false
      return { response: { status: 204 } }
    })
    mount()

    fireEvent.click(await screen.findByRole('button', { name: 'Sign out' }))

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/sessions/current'),
    )
    expect(await screen.findByRole('button', { name: 'Sign in' })).toBeTruthy()
  })

  // The address belongs to the session that made it. Another Person
  // who signs in on the same page opens Home, not the last Person's
  // conversation.
  it('opens Home at the next sign-in, not the last address', async () => {
    let current: typeof person | null = person
    api.GET.mockImplementation(async (path: string) => {
      if (path !== '/api/v1/user') return shellResponse(path)
      return current === null
        ? refused(401, 'sign in first')
        : { data: current, response: { status: 200 } }
    })
    api.DELETE.mockImplementation(async () => {
      current = null
      return { response: { status: 204 } }
    })
    const lin = { id: 'user-2', name: 'Lin', email: 'lin@example.com', role: 'member' }
    api.POST.mockImplementation(async () => {
      current = lin
      return { data: lin, response: { status: 200 } }
    })
    const history = mount('/c/channel-2')
    expect(await screen.findByRole('heading', { name: 'Launch planning' })).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Sign out' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Sign in' }))

    expect(await screen.findByTestId('home')).toBeTruthy()
    expect(history.location.pathname).toBe('/')
    expect(screen.queryByRole('heading', { name: 'Launch planning' })).toBeNull()
  })

  // The daemon closes the socket with 1008 when the Session ends
  // somewhere else: on another device, by an Administrator, or at its
  // expiry. The app shows the sign-in page and opens no socket again.
  it('gives the sign-in page back when the socket says the Session ended', async () => {
    let signedIn = true
    api.GET.mockImplementation(async (path: string) => {
      if (path !== '/api/v1/user') return shellResponse(path)
      return signedIn
        ? { data: person, response: { status: 200 } }
        : refused(401, 'sign in first')
    })
    mount()
    expect(await screen.findByRole('navigation', { name: 'Places' })).toBeTruthy()
    expect(sockets).toHaveLength(1)

    signedIn = false
    act(() => sockets[0].handlers.onSignedOut())

    expect(await screen.findByRole('button', { name: 'Sign in' })).toBeTruthy()
    expect(sockets[0].stopped).toBe(true)
    expect(sockets).toHaveLength(1)
    expect(api.DELETE).not.toHaveBeenCalled()
  })
})
