// The service worker of the Product App, loaded into a fake global scope
// that records its listeners.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

type Event = { waitUntil: (promise: Promise<unknown>) => void } & Record<string, unknown>
type Listener = (event: Event) => void

function fakeScope() {
  const listeners = new Map<string, Listener[]>()
  const scope = {
    listeners,
    skipWaiting: vi.fn(() => Promise.resolve()),
    clients: {
      claim: vi.fn(() => Promise.resolve()),
      matchAll: vi.fn(() => Promise.resolve([])),
      openWindow: vi.fn(() => Promise.resolve(null)),
    },
    registration: {
      showNotification: vi.fn((_title: string, _options?: NotificationOptions) => Promise.resolve()),
    },
    navigator: {},
    Notification: { maxActions: 2 },
    fetch: vi.fn(function (this: unknown, _url: string, _init?: RequestInit) {
      // A browser refuses a `fetch` that is not called on the scope.
      if (this !== scope) throw new TypeError('Illegal invocation')
      return Promise.resolve({ ok: true, status: 200 })
    }),
    location: { origin: 'https://pagis.example' },
    addEventListener(type: string, listener: Listener) {
      listeners.set(type, [...(listeners.get(type) ?? []), listener])
    },
    /** Run each listener of `type` with the members of `event`, and
     * return what it waits for. */
    dispatch(type: string, event: Record<string, unknown> = {}): Promise<unknown>[] {
      const waits: Promise<unknown>[] = []
      for (const listener of listeners.get(type) ?? []) {
        listener({ ...event, waitUntil: (promise) => waits.push(promise) })
      }
      return waits
    },
  }
  return scope
}

/** The plaintext that the daemon writes for a pending Approval. */
const approval = JSON.stringify({
  web_push: 8030,
  notification: {
    title: 'Ada',
    body: 'Ada wants to send an email',
    navigate: 'https://pagis.example/c/channel-1',
    data: {
      v: 1,
      item: 'request:req-1',
      kind: 'approval',
      request: { id: 'req-1', actions: ['approve_once', 'deny'] },
    },
  },
})

describe('the service worker', () => {
  let scope: ReturnType<typeof fakeScope>

  beforeEach(async () => {
    scope = fakeScope()
    vi.stubGlobal('self', scope)
    vi.resetModules()
    await import('./sw')
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  // It holds no cache and no state, so a new worker takes over at once.
  it('skips the wait at install', () => {
    scope.dispatch('install')
    expect(scope.skipWaiting).toHaveBeenCalledOnce()
  })

  it('claims the open pages at activate', async () => {
    const waits = scope.dispatch('activate')
    expect(scope.clients.claim).toHaveBeenCalledOnce()
    expect(waits).toHaveLength(1)
    await Promise.all(waits)
  })

  // The Product App needs the daemon, so the worker serves no request.
  it('has no fetch listener', () => {
    expect(scope.listeners.has('fetch')).toBe(false)
  })

  it('shows a Notification for a push, and waits for it', async () => {
    const waits = scope.dispatch('push', { data: null })
    expect(waits).toHaveLength(1)
    await Promise.all(waits)
    expect(scope.registration.showNotification).toHaveBeenCalledOnce()
  })

  it('shows the actions that the browser shows', async () => {
    await Promise.all(scope.dispatch('push', { data: { text: () => approval } }))
    const [[, options]] = scope.registration.showNotification.mock.calls
    expect((options as { actions?: unknown[] }).actions).toHaveLength(2)
  })

  it('posts the answer of an action, and waits for it', async () => {
    const notification = {
      title: 'Ada',
      tag: 'request:req-1',
      data: { request: { id: 'req-1' }, navigate: '/c/channel-1' },
      close: vi.fn(),
    }
    const waits = scope.dispatch('notificationclick', { action: 'deny', notification })
    expect(waits).toHaveLength(1)
    await Promise.all(waits)
    expect(scope.fetch).toHaveBeenCalledOnce()
    expect(notification.close).toHaveBeenCalledOnce()
    expect(scope.clients.openWindow).not.toHaveBeenCalled()
  })

  it('opens the place of a tapped Notification, and waits for it', async () => {
    const notification = { data: { navigate: '/runs/run-1' }, close: vi.fn() }
    const waits = scope.dispatch('notificationclick', { action: '', notification })
    expect(waits).toHaveLength(1)
    await Promise.all(waits)
    expect(scope.clients.openWindow).toHaveBeenCalledExactlyOnceWith(
      'https://pagis.example/runs/run-1',
    )
  })
})
