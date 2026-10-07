import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory } from '@tanstack/react-router'
import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from './api/client'
import type { SocketHandlers, SocketOptions } from './ws/socket'
import { App } from './App'
import { useCallInspector } from './state/stores'
import { shellResponse } from './test/appStub'

const { api, socket } = vi.hoisted(() => ({
  socket: { handlers: null as SocketHandlers | null },
  api: {
    GET: vi.fn(),
    POST: vi.fn(),
    PUT: vi.fn(),
    DELETE: vi.fn(),
  },
}))

vi.mock('./api/client', async () => {
  const actual = await vi.importActual<typeof import('./api/client')>('./api/client')
  return { ...actual, createApiClient: () => api as unknown as ApiClient }
})

/** The Capacitor runtime that the Mobile App puts in the page, and its
 *  `PagisShell` plugin. A browser has neither. */
const { shell } = vi.hoisted(() => ({
  shell: { native: false, sessionEnded: vi.fn(async () => {}) },
}))

vi.mock('@capacitor/core', () => ({
  Capacitor: { isNativePlatform: () => shell.native },
  registerPlugin: (name: string) =>
    name === 'PagisShell'
      ? { sessionEnded: shell.sessionEnded, addListener: async () => ({ remove: async () => {} }) }
      : {},
}))

vi.mock('./ws/socket', () => ({
  PagisSocket: class {
    constructor(options: SocketOptions) {
      socket.handlers = options.handlers
    }
    start() {}
    stop() {}
    subscribeChannel() {}
    activity() {
      return false
    }
  },
}))

function mount(path = '/c/channel-1') {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <App history={createMemoryHistory({ initialEntries: [path] })} />
    </QueryClientProvider>,
  )
}

beforeEach(() => {
  shell.native = false
  shell.sessionEnded.mockClear()
  useCallInspector.setState({ callId: null })
  api.GET.mockReset()
  api.GET.mockImplementation(async (path: string) => shellResponse(path))
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response('missing', { status: 404 })),
  )
})

describe('conversation desk', () => {
  // The Chief of Staff's channel owns its Desk Panel by the route
  // (ADR-0022): it is already open, and no toggle offers to
  // close it.
  it('opens the Desk Panel by itself in the Chief of Staff\u2019s channel', async () => {
    mount()

    expect(await screen.findByText('Say hello to Sage')).toBeTruthy()
    expect(await screen.findByTestId('desk-panel')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Desk panel' })).toBeNull()
  })

  it('opens the Desk Panel from the toggle of another conversation', async () => {
    mount('/c/channel-2')

    expect(
      await screen.findByRole('heading', { name: 'Launch planning' }),
    ).toBeTruthy()
    expect(screen.queryByTestId('desk-panel')).toBeNull()

    fireEvent.click(screen.getByRole('button', { name: 'Desk panel' }))

    expect(await screen.findByTestId('desk-panel')).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'Launch planning' })).toBeTruthy()
  })

  it('opens the conversation drawer and closes it after channel selection', async () => {
    mount()

    await screen.findByText('Say hello to Sage')
    fireEvent.click(screen.getByRole('button', { name: 'Open conversations' }))
    expect(screen.getByRole('button', { name: 'Close conversations' })).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: /Launch planning/ }))

    expect(
      await screen.findByRole('heading', { name: 'Launch planning' }),
    ).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Close conversations' })).toBeNull()
  })

  it('closes the conversation drawer after opening a workspace view', async () => {
    mount()

    await screen.findByText('Say hello to Sage')
    fireEvent.click(screen.getByRole('button', { name: 'Open conversations' }))
    const navigation = screen.getByRole('navigation', { name: 'Places' })
    fireEvent.click(within(navigation).getByRole('button', { name: 'Automations' }))

    expect(screen.queryByRole('button', { name: 'Close conversations' })).toBeNull()
    expect(await screen.findByRole('heading', { name: 'Automations' })).toBeTruthy()
  })

  // ADR-0022: listening is the reason the call inspector exists, so
  // navigation must not stop it. Call is a transient tenant of the
  // slot: it is absent until a Call opens it.
  it('keeps the call inspector open across navigation', async () => {
    mount()

    await screen.findByText('Say hello to Sage')
    expect(screen.queryByTestId('call-inspector')).toBeNull()

    useCallInspector.getState().open('call_1')
    expect(await screen.findByTestId('call-inspector')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: /Launch planning/ }))
    expect(screen.getByTestId('call-inspector')).toBeTruthy()

    const navigation = screen.getByRole('navigation', { name: 'Places' })
    fireEvent.click(within(navigation).getByRole('button', { name: 'Settings' }))
    expect(await screen.findByRole('heading', { name: 'Settings' })).toBeTruthy()
    expect(screen.getByTestId('call-inspector')).toBeTruthy()
  })

  // One slot, one tenant: opening the Desk panel gives the slot back, and
  // the strip in the Thread is the way back to the Call.
  it('gives the slot back when the user opens the Desk panel', async () => {
    mount('/c/channel-2')

    await screen.findByRole('heading', { name: 'Launch planning' })
    useCallInspector.getState().open('call_1')
    await screen.findByTestId('call-inspector')

    fireEvent.click(screen.getByRole('button', { name: 'Desk panel' }))
    expect(await screen.findByTestId('desk-panel')).toBeTruthy()
    expect(screen.queryByTestId('call-inspector')).toBeNull()
  })

  // The Desk Panel is the slot's default tenant, so a Call takes the
  // slot from it and gives it back when it closes (ADR-0022).
  it('takes the slot from the Desk Panel for a Call and gives it back', async () => {
    mount()

    await screen.findByTestId('desk-panel')
    useCallInspector.getState().open('call_1')

    expect(await screen.findByTestId('call-inspector')).toBeTruthy()
    expect(screen.queryByTestId('desk-panel')).toBeNull()

    fireEvent.click(screen.getByLabelText('Close the call'))

    expect(await screen.findByTestId('desk-panel')).toBeTruthy()
  })
})

describe('the Needs-You Queue', () => {
  /** One `needs_you.*` frame, as the event socket delivers it. */
  function needsYouFrame(type: 'needs_you.added' | 'needs_you.removed', payload: object) {
    act(() => socket.handlers!.onEvent({
      type,
      payload: {
        id: `event-${type}`,
        event_type: type,
        created_at: Date.now(),
        payload,
      },
    }))
  }

  it('adds and removes a row on the queue events, with no reload', async () => {
    const missed = {
      kind: 'call',
      id: 'call:missed-1',
      agent_id: 'agent-1',
      line: 'Sage missed a call from +14155550199',
      url: '/',
      at: Date.now(),
      call_id: 'missed-1',
      remote_e164: '+14155550199',
      left_message: false,
    }
    let items: unknown[] = []
    api.GET.mockImplementation(async (path: string) =>
      path === '/api/v1/needs-you'
        ? { data: { items, count: items.length } }
        : shellResponse(path),
    )
    mount('/')
    await screen.findByText('Nothing needs you.')

    items = [missed]
    needsYouFrame('needs_you.added', { item: missed, count: 1 })
    expect(await screen.findByText('Sage missed a call from +14155550199')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Call back' })).toBeTruthy()

    items = []
    needsYouFrame('needs_you.removed', { item_id: 'call:missed-1', count: 0 })
    expect(await screen.findByText('Nothing needs you.')).toBeTruthy()
    expect(screen.queryByText('Sage missed a call from +14155550199')).toBeNull()
  })
})

describe('sign-in link', () => {
  // The link opens its own page, also in a browser that holds a Session
  // already: the link signs it in as the person it was made for.
  it('opens the sign-in link page at its path, signed in or not', async () => {
    api.POST.mockClear()
    mount('/sign-in')

    expect(await screen.findByText(/holds no sign-in link/)).toBeTruthy()
    expect(screen.queryByTestId('desk-panel')).toBeNull()
    expect(api.POST).not.toHaveBeenCalled()
  })
})

describe('the sign-in page', () => {
  /** A browser with no Session, and the sign-in that the health answer
   *  names for it. */
  function signedOut(signIn: 'password' | 'link') {
    api.GET.mockImplementation(async (path: string) => {
      if (path === '/api/v1/user') {
        return { error: { error: { code: 'unauthorized', message: 'no session' } } }
      }
      if (path === '/api/v1/setup') {
        return { error: { error: { code: 'setup_complete', message: 'gone' } } }
      }
      if (path === '/api/v1/health') {
        return { data: { status: 'ok', version: '0.1.1', sign_in: signIn } }
      }
      return shellResponse(path)
    })
  }

  // In Remote Access the daemon takes no password from another machine,
  // so that browser gets the field for a Sign-In Link (ADR-0028).
  it('shows another machine in Remote Access the field for a sign-in link', async () => {
    signedOut('link')
    mount('/')

    expect(await screen.findByLabelText('Paste a sign-in link')).toBeTruthy()
    expect(screen.queryByLabelText('Password')).toBeNull()
  })

  it('shows this machine the address and the password', async () => {
    signedOut('password')
    mount('/')

    expect(await screen.findByLabelText('Password')).toBeTruthy()
    expect(screen.queryByLabelText('Paste a sign-in link')).toBeNull()
  })
})

describe('the end of a Session in the Mobile App', () => {
  /** The daemon closes the event socket with 1008 when the Session
   *  ended, and from then on refuses the read of the Person. */
  async function endTheSession() {
    await screen.findByText('Say hello to Sage')
    api.GET.mockImplementation(async (path: string) =>
      path === '/api/v1/user'
        ? { error: { error: { code: 'unauthorized', message: 'no session' } } }
        : shellResponse(path),
    )
    act(() => socket.handlers!.onSignedOut())
  }

  it('tells the Mobile App that the Session ended, so it opens the Connect screen', async () => {
    shell.native = true
    mount()

    await endTheSession()

    await vi.waitFor(() => expect(shell.sessionEnded).toHaveBeenCalledTimes(1))
  })

  it('tells nobody in a browser, which shows its sign-in page', async () => {
    mount()

    await endTheSession()

    expect(await screen.findByLabelText('Password')).toBeTruthy()
    expect(shell.sessionEnded).not.toHaveBeenCalled()
  })

  /** An address opens the sign-in page of the server in the Mobile App,
   *  and the Person signs in there. No Session ended. */
  it('tells the Mobile App nothing when a page opens with no Session', async () => {
    shell.native = true
    api.GET.mockImplementation(async (path: string) => {
      if (path === '/api/v1/user') {
        return { error: { error: { code: 'unauthorized', message: 'no session' } } }
      }
      if (path === '/api/v1/health') {
        return { data: { status: 'ok', version: '0.1.1', sign_in: 'password' } }
      }
      return shellResponse(path)
    })
    mount('/')

    expect(await screen.findByLabelText('Password')).toBeTruthy()
    expect(shell.sessionEnded).not.toHaveBeenCalled()
  })
})
