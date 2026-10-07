// The logic of the service worker: it shows the Notification of a push,
// keeps the app badge on the count of the queue, opens the place of a
// Notification that the Person taps, and sends the answer of an action
// button. A fake registration, fake clients, a fake `fetch` and a fake
// `Notification.maxActions` stand in for the browser.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { clickNotification, openPlace, showPush } from './handlers'

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

/** Safari has no `Notification.maxActions` and shows no action. */
const NO_ACTIONS = {}

/** Chrome and Edge show two actions. */
const TWO_ACTIONS = { maxActions: 2 }

describe('a push', () => {
  it('shows the Notification of the payload', async () => {
    const registration = fakeRegistration()

    await showPush(registration, {}, NO_ACTIONS, approval)

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

    await showPush(fakeRegistration(), navigator, NO_ACTIONS, approval)

    expect(navigator.setAppBadge).toHaveBeenCalledExactlyOnceWith(3)
  })

  // Chrome on Android has no Badging API.
  it('shows the Notification where the browser has no app badge', async () => {
    const registration = fakeRegistration()

    await expect(showPush(registration, {}, NO_ACTIONS, approval)).resolves.toBeUndefined()

    expect(registration.showNotification).toHaveBeenCalledOnce()
  })

  // The test Notification from Settings sets no badge.
  it('sets no badge for a payload with no badge', async () => {
    const navigator = badgeNavigator()
    const payload = JSON.parse(approval) as Record<string, unknown>
    delete payload.app_badge

    await showPush(fakeRegistration(), navigator, NO_ACTIONS, JSON.stringify(payload))

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

    await showPush(registration, navigator, NO_ACTIONS, text)

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

describe('the actions of a push', () => {
  /** The `actions` of the one Notification that the registration showed. */
  async function shownActions(notificationClass: { maxActions?: number }, text: string) {
    const registration = fakeRegistration()
    await showPush(registration, {}, notificationClass, text)
    const [[, options]] = registration.showNotification.mock.calls
    return (options as { actions?: unknown }).actions
  }

  it('shows Approve once and Deny on a tool action Approval where the browser shows two actions', async () => {
    expect(await shownActions(TWO_ACTIONS, approval)).toEqual([
      { action: 'approve_once', title: 'Approve once' },
      { action: 'deny', title: 'Deny' },
    ])
  })

  it.each([
    ['no actions', NO_ACTIONS],
    ['no actions at all', { maxActions: 0 }],
    ['one action', { maxActions: 1 }],
  ])('shows no action where the browser shows %s', async (_case, notificationClass) => {
    expect(await shownActions(notificationClass, approval)).toBeUndefined()
  })

  // A failed Run, a missed Call and the test Notification have no
  // Request to answer.
  it('shows no action for a payload with no request', async () => {
    const payload = JSON.parse(approval) as { notification: { data: Record<string, unknown> } }
    delete payload.notification.data.request
    payload.notification.data.kind = 'failed'

    expect(await shownActions(TWO_ACTIONS, JSON.stringify(payload))).toBeUndefined()
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

/** The Notification of the Approval, as the worker showed it. */
function approvalNotification() {
  return {
    title: 'Ada',
    tag: 'request:req-1',
    data: {
      v: 1,
      item: 'request:req-1',
      kind: 'approval',
      request: { id: 'req-1', actions: ['approve_once', 'deny'] },
      navigate: `${ORIGIN}/c/channel-1`,
    },
    close: vi.fn(),
  }
}

type Answer = { ok: boolean; status: number }

/** A `fetch` that answers each request with `answer`. */
function fakeFetch(answer: (init: RequestInit) => Promise<Answer>) {
  return vi.fn((_url: string, init: RequestInit) => answer(init))
}

function status(code: number): Promise<Answer> {
  return Promise.resolve({ ok: code >= 200 && code < 300, status: code })
}

function fakeWorker(fetch: ReturnType<typeof fakeFetch>) {
  return { clients: fakeClients([]), origin: ORIGIN, registration: fakeRegistration(), fetch }
}

/** The Notification that replaces one whose answer did not go through. */
const FAILURE = {
  body: 'Pagis did not take this answer. Open Pagis to see the request.',
  tag: 'request:req-1',
  data: { navigate: `${ORIGIN}/c/channel-1` },
  navigate: `${ORIGIN}/c/channel-1`,
  icon: '/icon-192.png',
  badge: '/badge-96.png',
}

describe('an action on a Notification', () => {
  beforeEach(() => {
    // A failed answer writes a log line.
    vi.spyOn(console, 'error').mockImplementation(() => {})
  })

  afterEach(() => {
    vi.restoreAllMocks()
    vi.useRealTimers()
  })

  // The worker sends the Session cookie of the browser. An answer from a
  // Notification approves once, so it sends no scope and writes no Allow
  // Rule.
  it.each([
    ['approve_once', 'approved'],
    ['deny', 'denied'],
  ])('%s posts %s to the decision route with the Session cookie and no scope', async (action, decision) => {
    const fetch = fakeFetch(() => status(200))

    await clickNotification(action, approvalNotification(), fakeWorker(fetch))

    expect(fetch).toHaveBeenCalledExactlyOnceWith('/api/v1/requests/req-1/decision', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ decision }),
      signal: expect.any(AbortSignal) as AbortSignal,
    })
  })

  it('closes the Notification when the daemon takes the answer', async () => {
    const notification = approvalNotification()
    const worker = fakeWorker(fakeFetch(() => status(200)))

    await clickNotification('approve_once', notification, worker)

    expect(notification.close).toHaveBeenCalledOnce()
    expect(worker.registration.showNotification).not.toHaveBeenCalled()
    expect(worker.clients.openWindow).not.toHaveBeenCalled()
  })

  // `409` is a Request that another client decided, and `401` is a
  // Session that ended.
  it.each([409, 401])('shows the failure in place of the Notification on a %i', async (code) => {
    const notification = approvalNotification()
    const worker = fakeWorker(fakeFetch(() => status(code)))

    await clickNotification('deny', notification, worker)

    expect(worker.registration.showNotification).toHaveBeenCalledExactlyOnceWith('Ada', FAILURE)
    expect(notification.close).not.toHaveBeenCalled()
  })

  it('shows the failure on a network failure', async () => {
    const worker = fakeWorker(fakeFetch(() => Promise.reject(new TypeError('Failed to fetch'))))

    await clickNotification('approve_once', approvalNotification(), worker)

    expect(worker.registration.showNotification).toHaveBeenCalledExactlyOnceWith('Ada', FAILURE)
  })

  it('stops the request and shows the failure after 20 seconds', async () => {
    vi.useFakeTimers()
    // The request goes on until its signal stops it, as `fetch` does.
    const fetch = fakeFetch(
      (init) =>
        new Promise<Answer>((_resolve, reject) => {
          init.signal?.addEventListener('abort', () => reject(init.signal?.reason))
        }),
    )
    const worker = fakeWorker(fetch)

    const click = clickNotification('approve_once', approvalNotification(), worker)
    await vi.advanceTimersByTimeAsync(19_999)
    expect(worker.registration.showNotification).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1)
    await click

    expect(worker.registration.showNotification).toHaveBeenCalledExactlyOnceWith('Ada', FAILURE)
  })

  it('opens the place of the item on a click on the body', async () => {
    const fetch = fakeFetch(() => status(200))
    const worker = fakeWorker(fetch)

    await clickNotification('', approvalNotification(), worker)

    expect(worker.clients.openWindow).toHaveBeenCalledExactlyOnceWith(`${ORIGIN}/c/channel-1`)
    expect(fetch).not.toHaveBeenCalled()
  })
})
