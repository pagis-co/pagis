// A tap on a Notification in the Mobile App, while the Product App is
// open, moves the router of the page. The shell sends the `navigate`
// event of `PagisShell` with the path of the place, and no page loads
// again (ADR-0032).

import {
  Outlet,
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  type RouterHistory,
} from '@tanstack/react-router'
import { act, render, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { appPath, phoneParent, useShellNavigation } from './mobileShell'

/** The Capacitor runtime that the Mobile App puts in the page, and the
 *  listeners of its `PagisShell` plugin. A browser has neither. */
const { shell } = vi.hoisted(() => ({
  shell: {
    native: false,
    platform: 'web',
    listeners: new Map<string, (event: unknown) => void>(),
    removed: [] as string[],
  },
}))

vi.mock('@capacitor/core', () => ({
  Capacitor: { isNativePlatform: () => shell.native, getPlatform: () => shell.platform },
  registerPlugin: () => ({
    addListener: vi.fn(async (eventName: string, listener: (event: unknown) => void) => {
      shell.listeners.set(eventName, listener)
      return { remove: async () => void shell.removed.push(eventName) }
    }),
  }),
}))

// The Android back button of the Mobile App.
vi.mock('@capacitor/app', () => ({
  App: {
    addListener: vi.fn(async (eventName: string, listener: (event: unknown) => void) => {
      shell.listeners.set(eventName, listener)
      return { remove: async () => void shell.removed.push(eventName) }
    }),
  },
}))

function mount(url = '/'): { history: RouterHistory; unmount: () => void } {
  const history = createMemoryHistory({ initialEntries: [url] })
  const rootRoute = createRootRoute({
    component: () => {
      useShellNavigation()
      return <Outlet />
    },
  })
  const routes = ['/', '/c/$channelId', '/runs/$runId', '/sprites/$agentId/work', '/sprites/$agentId/desk', '/you', '/memory'].map((path) =>
    createRoute({ getParentRoute: () => rootRoute, path, component: () => null }),
  )
  const router = createRouter({ routeTree: rootRoute.addChildren(routes), history })
  const { unmount } = render(<RouterProvider router={router as never} />)
  return { history, unmount }
}

/** Send the `navigate` event of `PagisShell`, as the shell does. */
function shellSends(event: unknown): void {
  const listener = shell.listeners.get('navigate')
  if (listener === undefined) throw new Error('The page listens for no navigate event.')
  act(() => listener(event))
}

describe('useShellNavigation', () => {
  beforeEach(() => {
    shell.native = false
    shell.platform = 'web'
    shell.listeners.clear()
    shell.removed = []
  })

  it('moves the router to the path of the navigate event', async () => {
    shell.native = true
    const { history } = mount()
    await waitFor(() => expect(shell.listeners.has('navigate')).toBe(true))

    shellSends({ path: '/c/channel-1?card=r-1#approval' })

    await waitFor(() => expect(history.location.pathname).toBe('/c/channel-1'))
    expect(history.location.search).toBe('?card=r-1')
    expect(history.location.hash).toBe('#approval')
  })

  it('keeps the page on its origin', async () => {
    shell.native = true
    const { history } = mount()
    await waitFor(() => expect(shell.listeners.has('navigate')).toBe(true))

    shellSends({ path: 'https://evil.example/c/channel-1' })
    shellSends({ path: '//evil.example/c/channel-1' })
    shellSends({})

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(history.location.pathname).toBe('/')
  })

  it('removes its listener when the page unmounts', async () => {
    shell.native = true
    const { unmount } = mount()
    await waitFor(() => expect(shell.listeners.has('navigate')).toBe(true))

    unmount()

    await waitFor(() => expect(shell.removed).toEqual(['navigate']))
  })

  it('adds no listener in a browser', async () => {
    mount()
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })

    expect(shell.listeners.size).toBe(0)
  })
})

describe('appPath', () => {
  it('keeps a path, its query and its fragment on this app', () => {
    expect(appPath('/c/channel-1?card=r-1#approval')).toBe('/c/channel-1?card=r-1#approval')
    expect(appPath('/sprites/agent-1/work')).toBe('/sprites/agent-1/work')
  })

  it.each([
    ['another origin', 'https://evil.example/c/channel-1'],
    ['a protocol-relative address', '//evil.example/c/channel-1'],
    ['a backslash after the slash', '/\\evil.example/c/channel-1'],
    ['a tab between the slashes', '/\t/evil.example'],
    ['a script', 'javascript:alert(1)'],
    ['a relative path', 'c/channel-1'],
    ['no string', 42],
  ])('drops %s', (_, value) => {
    expect(appPath(value)).toBeUndefined()
  })
})

describe('phoneParent', () => {
  it.each([
    ['/sprites/agent-1', undefined, 'Sprites', '/sprites'],
    ['/sprites/agent-1/work', undefined, 'Sprite', '/sprites/agent-1'],
    ['/sprites/agent-1/access/connection-1', undefined, 'Access', '/sprites/agent-1/access'],
    ['/sprites/agent-1/desk', '/runs/run-1', 'Run', '/runs/run-1'],
    ['/sprites/agent-1/desk', '/c/channel-1', 'Conversation', '/c/channel-1'],
    ['/sprites/agent-1/desk', undefined, 'Sprite', '/sprites/agent-1'],
    ['/memory', '/sprites/agent-1/memory', 'Sprite', '/sprites/agent-1/memory'],
    ['/memory', undefined, 'You', '/you'],
    ['/c/channel-1/t/message-1', undefined, 'Conversation', '/c/channel-1'],
    ['/c/channel-1', undefined, 'Conversations', '/conversations'],
    ['/runs/run-1', undefined, 'Home', '/'],
    ['/runs/run-1', '/sprites/agent-1/work', 'Work', '/sprites/agent-1/work'],
    ['/calls/call-1', undefined, 'Home', '/'],
    ['/settings/connections/connection-1', undefined, 'Connections', '/settings/connections'],
    ['/settings/trusted-contacts/keypad', undefined, 'Trusted contacts', '/settings/trusted-contacts'],
    ['/settings/models', undefined, 'Settings', '/settings'],
    ['/settings', undefined, 'You', '/you'],
    ['/automations', undefined, 'You', '/you'],
    ['/coding/session-1', undefined, 'Coding', '/coding'],
  ])('gives %s from %s the parent %s', (path, from, label, parent) => {
    expect(phoneParent(path, from)).toEqual({ label, path: parent })
  })

  it('ignores a from that leaves this app', () => {
    expect(phoneParent('/sprites/agent-1/desk', '//evil.example')).toEqual({ label: 'Sprite', path: '/sprites/agent-1' })
    expect(phoneParent('/runs/run-1', '/\\evil.example')).toEqual({ label: 'Home', path: '/' })
  })
})

describe('the Android back button', () => {
  beforeEach(() => {
    shell.native = true
    shell.platform = 'android'
    shell.listeners.clear()
    shell.removed = []
    vi.stubGlobal('matchMedia', (query: string) => ({
      matches: query.includes('max-width'),
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    }))
  })
  afterEach(() => vi.unstubAllGlobals())

  function pressBack(): void {
    const listener = shell.listeners.get('backButton')
    if (listener === undefined) throw new Error('The page listens for no back button.')
    act(() => listener({}))
  }

  it('goes to the fixed parent of the screen', async () => {
    const { history } = mount('/c/channel-1')
    await waitFor(() => expect(shell.listeners.has('backButton')).toBe(true))

    pressBack()

    await waitFor(() => expect(history.location.pathname).toBe('/conversations'))
  })

  it('goes to the place in from', async () => {
    const { history } = mount('/runs/run-1?from=%2Fsprites%2Fagent-1%2Fwork')
    await waitFor(() => expect(shell.listeners.has('backButton')).toBe(true))

    pressBack()

    await waitFor(() => expect(history.location.pathname).toBe('/sprites/agent-1/work'))
  })

  it('stays on this app when from names another site', async () => {
    const { history } = mount('/sprites/agent-1/desk?from=%2F%2Fevil.example')
    await waitFor(() => expect(shell.listeners.has('backButton')).toBe(true))

    pressBack()

    await waitFor(() => expect(history.location.pathname).toBe('/sprites/agent-1'))
  })

  it('closes the open sheet first', async () => {
    const { history } = mount('/c/channel-1')
    await waitFor(() => expect(shell.listeners.has('backButton')).toBe(true))
    const sheet = document.createElement('div')
    sheet.setAttribute('role', 'dialog')
    document.body.append(sheet)
    const escape = vi.fn()
    document.addEventListener('keydown', escape)

    pressBack()

    expect(escape).toHaveBeenCalledWith(expect.objectContaining({ key: 'Escape' }))
    expect(history.location.pathname).toBe('/c/channel-1')
    document.removeEventListener('keydown', escape)
    sheet.remove()
  })

  it('does nothing on Home', async () => {
    const { history } = mount('/')
    await waitFor(() => expect(shell.listeners.has('backButton')).toBe(true))

    pressBack()

    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    expect(history.location.pathname).toBe('/')
  })
})
