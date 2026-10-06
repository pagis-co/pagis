// The service worker of the Product App, loaded into a fake global scope
// that records its listeners.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

type Listener = (event: { waitUntil: (promise: Promise<unknown>) => void }) => void

function fakeScope() {
  const listeners = new Map<string, Listener[]>()
  return {
    listeners,
    skipWaiting: vi.fn(() => Promise.resolve()),
    clients: { claim: vi.fn(() => Promise.resolve()) },
    addEventListener(type: string, listener: Listener) {
      listeners.set(type, [...(listeners.get(type) ?? []), listener])
    },
    /** Run each listener of `type`, and return what it waits for. */
    dispatch(type: string): Promise<unknown>[] {
      const waits: Promise<unknown>[] = []
      for (const listener of listeners.get(type) ?? []) {
        listener({ waitUntil: (promise) => waits.push(promise) })
      }
      return waits
    },
  }
}

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

  it('listens for push and for a click on a notification', () => {
    expect(scope.listeners.get('push')).toHaveLength(1)
    expect(scope.listeners.get('notificationclick')).toHaveLength(1)
  })
})
