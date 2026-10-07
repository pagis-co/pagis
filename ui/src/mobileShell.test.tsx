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
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { useShellNavigation } from './mobileShell'

/** The Capacitor runtime that the Mobile App puts in the page, and the
 *  listeners of its `PagisShell` plugin. A browser has neither. */
const { shell } = vi.hoisted(() => ({
  shell: {
    native: false,
    listeners: new Map<string, (event: unknown) => void>(),
    removed: [] as string[],
  },
}))

vi.mock('@capacitor/core', () => ({
  Capacitor: { isNativePlatform: () => shell.native },
  registerPlugin: () => ({
    addListener: vi.fn(async (eventName: string, listener: (event: unknown) => void) => {
      shell.listeners.set(eventName, listener)
      return { remove: async () => void shell.removed.push(eventName) }
    }),
  }),
}))

function mount(): { history: RouterHistory; unmount: () => void } {
  const history = createMemoryHistory({ initialEntries: ['/'] })
  const rootRoute = createRootRoute({
    component: () => {
      useShellNavigation()
      return <Outlet />
    },
  })
  const routes = ['/', '/c/$channelId'].map((path) =>
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
