// The logic of the service worker: it shows the Notification of a push,
// keeps the app badge on the count of the queue, and opens the place of
// a Notification that the Person taps. A fake registration and fake
// clients stand in for the browser.

import { describe, expect, it, vi } from 'vitest'

import { openPlace, showPush } from './handlers'

const ORIGIN = 'https://pagis.example'

/** The plaintext that the daemon writes for a pending Approval. */
const approval = JSON.stringify({
  web_push: 8030,
  notification: {
    title: 'Ada',
    body: 'Ada wants to send an email\nSend the weekly report',
    navigate: `${ORIGIN}/c/channel-1`,
    data: {
      v: 1,
      item: 'request:req-1',
      kind: 'approval',
      request: { id: 'req-1', actions: ['approve_once', 'deny'] },
    },
  },
  mutable: true,
  app_badge: 3,
})

function fakeRegistration() {
  return { showNotification: vi.fn((_title: string, _options?: NotificationOptions) => Promise.resolve()) }
}

function badgeNavigator() {
  return { setAppBadge: vi.fn((_count?: number) => Promise.resolve()) }
}

describe('a push', () => {
  it('shows the Notification of the payload', async () => {
    const registration = fakeRegistration()

    await showPush(registration, {}, approval)

    expect(registration.showNotification).toHaveBeenCalledExactlyOnceWith('Ada', {
      body: 'Ada wants to send an email\nSend the weekly report',
      tag: 'request:req-1',
      data: {
        v: 1,
        item: 'request:req-1',
        kind: 'approval',
        request: { id: 'req-1', actions: ['approve_once', 'deny'] },
        navigate: `${ORIGIN}/c/channel-1`,
      },
      navigate: `${ORIGIN}/c/channel-1`,
      icon: '/icon-192.png',
      badge: '/badge-96.png',
    })
  })

  it('sets the app badge to the count of the payload', async () => {
    const navigator = badgeNavigator()

    await showPush(fakeRegistration(), navigator, approval)

    expect(navigator.setAppBadge).toHaveBeenCalledExactlyOnceWith(3)
  })

  // Chrome on Android has no Badging API.
  it('shows the Notification where the browser has no app badge', async () => {
    const registration = fakeRegistration()

    await expect(showPush(registration, {}, approval)).resolves.toBeUndefined()

    expect(registration.showNotification).toHaveBeenCalledOnce()
  })

  // The test Notification from Settings sets no badge.
  it('sets no badge for a payload with no badge', async () => {
    const navigator = badgeNavigator()
    const payload = JSON.parse(approval) as Record<string, unknown>
    delete payload.app_badge

    await showPush(fakeRegistration(), navigator, JSON.stringify(payload))

    expect(navigator.setAppBadge).not.toHaveBeenCalled()
  })

  // A browser can end a subscription whose push shows no Notification,
  // so every push shows one.
  it.each([
    ['a payload that does not parse', 'not json'],
    ['no data', undefined],
    ['a payload of another version', approval.replace('"v":1', '"v":2')],
    ['a payload with no notification', JSON.stringify({ web_push: 8030 })],
  ])('shows a placeholder for %s', async (_case, text) => {
    const registration = fakeRegistration()
    const navigator = badgeNavigator()

    await showPush(registration, navigator, text)

    expect(registration.showNotification).toHaveBeenCalledExactlyOnceWith('Pagis', {
      body: 'Something needs you',
      data: { navigate: '/' },
      navigate: '/',
      icon: '/icon-192.png',
      badge: '/badge-96.png',
    })
    expect(navigator.setAppBadge).not.toHaveBeenCalled()
  })
})

function fakeNotification(data: unknown) {
  return { data, close: vi.fn() }
}

function fakeWindow() {
  return { focus: vi.fn(() => Promise.resolve()), postMessage: vi.fn() }
}

function fakeClients(windows: ReturnType<typeof fakeWindow>[]) {
  return {
    matchAll: vi.fn((_options?: ClientQueryOptions) => Promise.resolve(windows)),
    openWindow: vi.fn((_url: string | URL) => Promise.resolve(null)),
  }
}

describe('a click on a Notification', () => {
  it('closes the Notification', async () => {
    const notification = fakeNotification({ navigate: `${ORIGIN}/c/channel-1` })

    await openPlace(notification, fakeClients([]), ORIGIN)

    expect(notification.close).toHaveBeenCalledOnce()
  })

  // `WindowClient.navigate()` loads the page again and loses its state,
  // so the page moves its own router.
  it('focuses an open window and tells it the place', async () => {
    const window = fakeWindow()
    const clients = fakeClients([window])

    await openPlace(fakeNotification({ navigate: `${ORIGIN}/c/channel-1` }), clients, ORIGIN)

    expect(clients.matchAll).toHaveBeenCalledExactlyOnceWith({
      type: 'window',
      includeUncontrolled: true,
    })
    expect(window.focus).toHaveBeenCalledOnce()
    expect(window.postMessage).toHaveBeenCalledExactlyOnceWith({
      type: 'navigate',
      url: `${ORIGIN}/c/channel-1`,
    })
    expect(clients.openWindow).not.toHaveBeenCalled()
  })

  it('opens a window at the place when no window is open', async () => {
    const clients = fakeClients([])

    await openPlace(fakeNotification({ navigate: `${ORIGIN}/runs/run-1` }), clients, ORIGIN)

    expect(clients.openWindow).toHaveBeenCalledExactlyOnceWith(`${ORIGIN}/runs/run-1`)
  })

  it('opens the root for a place on another origin', async () => {
    const clients = fakeClients([])

    await openPlace(fakeNotification({ navigate: 'https://evil.example/c/1' }), clients, ORIGIN)

    expect(clients.openWindow).toHaveBeenCalledExactlyOnceWith(`${ORIGIN}/`)
  })

  // The placeholder of a payload that did not parse names `/`.
  it('opens the root for a Notification with no place', async () => {
    const clients = fakeClients([])

    await openPlace(fakeNotification(null), clients, ORIGIN)

    expect(clients.openWindow).toHaveBeenCalledExactlyOnceWith(`${ORIGIN}/`)
  })
})
