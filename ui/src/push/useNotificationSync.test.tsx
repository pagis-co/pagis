// The open page keeps the Notifications and the app badge on the
// daemon's Needs-You Queue, and moves its router to the place that the
// service worker names after a tap on a Notification.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
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

import type { ApiClient, NeedsYouQueue } from '../api/client'
import { needsYouKey } from '../queries'
import { useNotificationSync } from './useNotificationSync'

function item(id: string): NeedsYouQueue['items'][number] {
  return { kind: 'failed', id, line: 'A Run failed', url: '/', at: 1, agent_id: 'ag-1', run_id: id }
}

function queue(...ids: string[]): NeedsYouQueue {
  return { items: ids.map(item), count: ids.length }
}

function fakeNotification(tag: string) {
  return { tag, close: vi.fn() }
}

/** The service worker container of the page: an event target for the
 * messages of the worker, and the registration that it answers. */
function giveServiceWorker(registration: { getNotifications: () => Promise<unknown[]> } | undefined) {
  const container = Object.assign(new EventTarget(), {
    getRegistration: vi.fn(() => Promise.resolve(registration)),
  })
  Object.defineProperty(navigator, 'serviceWorker', { configurable: true, value: container })
  return container
}

const setAppBadge = vi.fn((_count?: number) => Promise.resolve())
const clearAppBadge = vi.fn(() => Promise.resolve())

function giveBadge(): void {
  Object.defineProperty(navigator, 'setAppBadge', { configurable: true, value: setAppBadge })
  Object.defineProperty(navigator, 'clearAppBadge', { configurable: true, value: clearAppBadge })
}

function mount(first: NeedsYouQueue): { queryClient: QueryClient; history: RouterHistory } {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const api = {
    GET: vi.fn(async () => ({ data: first, error: undefined })),
  } as unknown as ApiClient
  const history = createMemoryHistory({ initialEntries: ['/'] })
  const rootRoute = createRootRoute({
    component: () => {
      useNotificationSync(api)
      return <Outlet />
    },
  })
  const routes = ['/', '/c/$channelId'].map((path) =>
    createRoute({ getParentRoute: () => rootRoute, path, component: () => null }),
  )
  const router = createRouter({ routeTree: rootRoute.addChildren(routes), history })
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router as never} />
    </QueryClientProvider>,
  )
  return { queryClient, history }
}

/** Let the pending promises of the hook settle. */
async function settle(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
}

describe('useNotificationSync', () => {
  beforeEach(() => {
    giveBadge()
  })

  afterEach(() => {
    Reflect.deleteProperty(navigator, 'serviceWorker')
    Reflect.deleteProperty(navigator, 'setAppBadge')
    Reflect.deleteProperty(navigator, 'clearAppBadge')
    setAppBadge.mockClear()
    clearAppBadge.mockClear()
  })

  it('closes a Notification whose item left the queue and keeps one whose item is in it', async () => {
    const gone = fakeNotification('run:old')
    const current = fakeNotification('request:req-1')
    giveServiceWorker({ getNotifications: () => Promise.resolve([gone, current]) })

    mount(queue('request:req-1'))

    await waitFor(() => expect(gone.close).toHaveBeenCalledOnce())
    expect(current.close).not.toHaveBeenCalled()
  })

  it('closes a Notification when its item leaves the queue later', async () => {
    const notification = fakeNotification('request:req-1')
    giveServiceWorker({ getNotifications: () => Promise.resolve([notification]) })
    const { queryClient } = mount(queue('request:req-1'))
    await waitFor(() => expect(setAppBadge).toHaveBeenCalledWith(1))
    expect(notification.close).not.toHaveBeenCalled()

    act(() => queryClient.setQueryData(needsYouKey, queue()))

    await waitFor(() => expect(notification.close).toHaveBeenCalledOnce())
  })

  it('sets the app badge to the count of the queue, and clears it at 0', async () => {
    giveServiceWorker({ getNotifications: () => Promise.resolve([]) })
    const { queryClient } = mount(queue('request:req-1', 'run:run-1'))

    await waitFor(() => expect(setAppBadge).toHaveBeenLastCalledWith(2))

    act(() => queryClient.setQueryData(needsYouKey, queue('run:run-1')))
    await waitFor(() => expect(setAppBadge).toHaveBeenLastCalledWith(1))

    act(() => queryClient.setQueryData(needsYouKey, queue()))
    await waitFor(() => expect(clearAppBadge).toHaveBeenCalledOnce())
  })

  // Chrome on Android has no Badging API.
  it('keeps the Notifications on the queue where the browser has no app badge', async () => {
    Reflect.deleteProperty(navigator, 'setAppBadge')
    Reflect.deleteProperty(navigator, 'clearAppBadge')
    const gone = fakeNotification('run:old')
    giveServiceWorker({ getNotifications: () => Promise.resolve([gone]) })

    mount(queue())

    await waitFor(() => expect(gone.close).toHaveBeenCalledOnce())
  })

  it('moves the router to the place that the service worker names', async () => {
    const container = giveServiceWorker({ getNotifications: () => Promise.resolve([]) })
    const { history } = mount(queue())
    await waitFor(() => expect(clearAppBadge).toHaveBeenCalled())

    act(() => {
      container.dispatchEvent(
        new MessageEvent('message', {
          data: { type: 'navigate', url: `${window.location.origin}/c/channel-1` },
        }),
      )
    })

    await waitFor(() => expect(history.location.pathname).toBe('/c/channel-1'))
  })

  it('ignores a message that is not a navigate message', async () => {
    const container = giveServiceWorker({ getNotifications: () => Promise.resolve([]) })
    const { history } = mount(queue())
    await waitFor(() => expect(clearAppBadge).toHaveBeenCalled())

    act(() => {
      container.dispatchEvent(new MessageEvent('message', { data: { type: 'other', url: '/c/1' } }))
    })
    await settle()

    expect(history.location.pathname).toBe('/')
  })

  // A development build, a page on an origin that is not secure, and the
  // Mobile App register no service worker.
  it('does nothing when the page has no service worker registration', async () => {
    const container = giveServiceWorker(undefined)
    const { history } = mount(queue('request:req-1'))
    await waitFor(() => expect(container.getRegistration).toHaveBeenCalled())
    await settle()

    act(() => {
      container.dispatchEvent(
        new MessageEvent('message', {
          data: { type: 'navigate', url: `${window.location.origin}/c/channel-1` },
        }),
      )
    })
    await settle()

    expect(setAppBadge).not.toHaveBeenCalled()
    expect(clearAppBadge).not.toHaveBeenCalled()
    expect(history.location.pathname).toBe('/')
  })

  it('does nothing where the browser has no service worker', async () => {
    mount(queue('request:req-1'))
    await settle()

    expect(setAppBadge).not.toHaveBeenCalled()
  })
})
