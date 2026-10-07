// The Notifications section: the state of this browser, the turn-on and
// the turn-off with a fake Push API or a fake `PagisPush` plugin of the
// Mobile App, and the list of every Push Subscription of the Person with
// a fake daemon.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient, PushSubscriptionDto } from '../../api/client'
import { Notifications } from './Notifications'

/** The fake `PagisPush` plugin of the Mobile App. */
const { pagisPush } = vi.hoisted(() => ({
  pagisPush: { state: vi.fn(), subscribe: vi.fn(), unsubscribe: vi.fn() },
}))

vi.mock('../../push/pagisPush', () => ({ PagisPush: pagisPush }))

/** The daemon's VAPID public key: an uncompressed P-256 point. */
const KEY_BYTES = Uint8Array.from([4, ...Array.from({ length: 64 }, (_, index) => index)])
const KEY = Buffer.from(KEY_BYTES).toString('base64url')
const OTHER_KEY_BYTES = Uint8Array.from([4, ...Array.from({ length: 64 }, () => 9)])

const THIS_BROWSER: PushSubscriptionDto = {
  id: 'ps-this',
  client_kind: 'browser',
  client_name: 'Firefox on Linux',
  created_at: 1_700_000_000_000,
  last_sent_at: 1_700_000_100_000,
  current: true,
}

// The Mobile App holds a `browser` Session, named by the app.
const PHONE: PushSubscriptionDto = {
  id: 'ps-phone',
  client_kind: 'browser',
  client_name: 'Pagis on iPhone',
  created_at: 1_700_000_000_000,
  last_sent_at: null,
  current: false,
}

const SUBSCRIPTION_JSON = {
  endpoint: 'https://push.example.com/send/abc',
  expirationTime: null,
  keys: { p256dh: 'BCVxsr7N', auth: 'c2VjcmV0' },
}

/** A Push Subscription that the fake browser holds. */
function fakeSubscription(key: Uint8Array) {
  return {
    endpoint: SUBSCRIPTION_JSON.endpoint,
    options: { applicationServerKey: key.slice().buffer, userVisibleOnly: true },
    toJSON: () => SUBSCRIPTION_JSON,
    unsubscribe: vi.fn(async () => {
      pushManager.held = null
      return true
    }),
  }
}

type FakeSubscription = ReturnType<typeof fakeSubscription>

/** The fake Push API of the browser. `subscribe` makes a subscription
 *  with the key it gets. */
const pushManager = {
  held: null as FakeSubscription | null,
  getSubscription: vi.fn(async () => pushManager.held),
  subscribe: vi.fn(async (options: { applicationServerKey: Uint8Array }) => {
    pushManager.held = fakeSubscription(options.applicationServerKey)
    return pushManager.held
  }),
}

function stubApi(items: PushSubscriptionDto[]) {
  let rows = items
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/push/key') return { data: { vapid_public_key: KEY } }
      if (path === '/api/v1/push-subscriptions') return { data: { items: rows } }
      throw new Error(`unexpected GET ${path}`)
    }),
    POST: vi.fn(async (path: string) => {
      if (path === '/api/v1/push-subscriptions') {
        rows = [...rows.filter((row) => !row.current), THIS_BROWSER]
        return { data: THIS_BROWSER }
      }
      if (path === '/api/v1/push-subscriptions/{push_subscription_id}/test') {
        return { data: { outcome: 'delivered' } }
      }
      throw new Error(`unexpected POST ${path}`)
    }),
    DELETE: vi.fn(async (_path: string, init: { params: { path: { push_subscription_id: string } } }) => {
      rows = rows.filter((row) => row.id !== init.params.path.push_subscription_id)
      return { response: new Response(null, { status: 204 }) }
    }),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={queryClient}>
      <Notifications api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
}

function setNavigator(name: string, value: unknown) {
  Object.defineProperty(navigator, name, { configurable: true, value })
}

beforeEach(() => {
  pushManager.held = null
  setNavigator('serviceWorker', { ready: Promise.resolve({ pushManager }) })
  vi.stubGlobal('PushManager', function PushManager() {})
  vi.stubGlobal('Notification', { permission: 'default' })
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.clearAllMocks()
  for (const name of ['serviceWorker', 'userAgent', 'standalone']) {
    Reflect.deleteProperty(navigator, name)
  }
})

describe('Notifications', () => {
  it('says that the Client App shows no notifications', async () => {
    setNavigator(
      'userAgent',
      'Mozilla/5.0 (Macintosh) AppleWebKit/537.36 (KHTML, like Gecko) Pagis/0.1.1 Chrome/152.0 Electron/44.1.1 Safari/537.36',
    )
    mount(stubApi([PHONE]))

    expect(
      await screen.findByText(
        'This app does not show notifications. Turn them on in a browser on your phone or your computer.',
      ),
    ).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Turn on' })).toBeNull()
  })

  it('tells a Safari tab on iOS to add Pagis to the Home Screen first', async () => {
    setNavigator('standalone', false)
    vi.stubGlobal('PushManager', undefined)
    mount(stubApi([]))

    expect(await screen.findByText('Add to Home Screen')).toBeTruthy()
    expect(
      screen.getByText('Open Pagis from the Home Screen and turn on notifications here.'),
    ).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Turn on' })).toBeNull()
  })

  it('tells how to allow notifications when the browser blocks them', async () => {
    vi.stubGlobal('Notification', { permission: 'denied' })
    mount(stubApi([]))

    expect(await screen.findByText(/This browser blocks notifications from this site/)).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Turn on' })).toBeNull()
  })

  it('shows Turn on while this browser gets no notifications', async () => {
    mount(stubApi([PHONE]))

    expect(await screen.findByRole('button', { name: 'Turn on' })).toBeTruthy()
    expect(screen.getByText('This browser does not get notifications.')).toBeTruthy()
  })

  it('shows Turn off while this browser gets notifications', async () => {
    pushManager.held = fakeSubscription(KEY_BYTES)
    mount(stubApi([THIS_BROWSER]))

    expect(await screen.findByRole('button', { name: 'Turn off' })).toBeTruthy()
    expect(screen.getByText('This browser gets notifications.')).toBeTruthy()
  })

  // A network wait inside the click can end the user gesture in Safari,
  // and then `subscribe()` fails. So the key loads before the click, and
  // the click calls `subscribe()` before it returns.
  it('subscribes with the key that loaded before the click, then posts the subscription', async () => {
    const api = stubApi([])
    mount(api)
    const turnOn = await screen.findByRole('button', { name: 'Turn on' })
    const keyReads = api.GET.mock.calls.filter(([path]) => path === '/api/v1/push/key').length
    expect(keyReads).toBe(1)

    fireEvent.click(turnOn)

    expect(pushManager.subscribe).toHaveBeenCalledExactlyOnceWith({
      userVisibleOnly: true,
      applicationServerKey: KEY_BYTES,
    })
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/push-subscriptions', {
        body: SUBSCRIPTION_JSON,
      }),
    )
    expect(await screen.findByRole('button', { name: 'Turn off' })).toBeTruthy()
    expect(api.GET.mock.calls.filter(([path]) => path === '/api/v1/push/key')).toHaveLength(1)
  })

  it('unsubscribes this browser and deletes its row on Turn off', async () => {
    const held = fakeSubscription(KEY_BYTES)
    pushManager.held = held
    const api = stubApi([PHONE, THIS_BROWSER])
    mount(api)

    await userEvent.click(await screen.findByRole('button', { name: 'Turn off' }))

    expect(held.unsubscribe).toHaveBeenCalledOnce()
    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/push-subscriptions/{push_subscription_id}', {
        params: { path: { push_subscription_id: 'ps-this' } },
      }),
    )
    expect(await screen.findByRole('button', { name: 'Turn on' })).toBeTruthy()
  })

  // The daemon's list is the truth.
  it('unsubscribes a subscription with another key and shows Off', async () => {
    const held = fakeSubscription(OTHER_KEY_BYTES)
    pushManager.held = held
    mount(stubApi([THIS_BROWSER]))

    expect(await screen.findByRole('button', { name: 'Turn on' })).toBeTruthy()
    await waitFor(() => expect(held.unsubscribe).toHaveBeenCalledOnce())
  })

  it('unsubscribes a subscription that the daemon does not list and shows Off', async () => {
    const held = fakeSubscription(KEY_BYTES)
    pushManager.held = held
    mount(stubApi([PHONE]))

    expect(await screen.findByRole('button', { name: 'Turn on' })).toBeTruthy()
    await waitFor(() => expect(held.unsubscribe).toHaveBeenCalledOnce())
  })

  it('lists each Push Subscription by its Session, with this browser named so', async () => {
    pushManager.held = fakeSubscription(KEY_BYTES)
    mount(stubApi([PHONE, THIS_BROWSER]))

    const rows = await screen.findAllByTestId('push-subscription-row')
    expect(rows).toHaveLength(2)
    expect(within(rows[0]).getByText('Pagis on iPhone')).toBeTruthy()
    expect(within(rows[0]).getByText('nothing sent yet')).toBeTruthy()
    expect(within(rows[1]).getByText('This browser')).toBeTruthy()
    expect(
      within(rows[1]).getByText(`last sent ${new Date(1_700_000_100_000).toLocaleString()}`),
    ).toBeTruthy()
  })

  it('sends a test to one Push Subscription', async () => {
    const api = stubApi([PHONE])
    mount(api)

    await userEvent.click(
      await screen.findByRole('button', { name: 'Send a test to Pagis on iPhone' }),
    )

    expect(api.POST).toHaveBeenCalledWith(
      '/api/v1/push-subscriptions/{push_subscription_id}/test',
      { params: { path: { push_subscription_id: 'ps-phone' } } },
    )
    expect(await screen.findByText('Sent.')).toBeTruthy()
  })

  describe('in the Mobile App', () => {
    /** The Push Subscription of the Session of the app. */
    const THIS_APP: PushSubscriptionDto = { ...PHONE, id: 'ps-app', current: true }

    /** What `PagisPush.subscribe` answers: the endpoint of the Push Relay
     *  and the public keys of the app. */
    const APP_SUBSCRIPTION = {
      endpoint: 'https://push-relay.example/v1/push/abc',
      keys: { p256dh: 'BCVxsr7N', auth: 'c2VjcmV0' },
    }

    const steps: string[] = []
    /** The service worker registration. The Mobile App has none, so the
     *  section must not wait for it. */
    const ready = vi.fn()

    /** A daemon that lists the Push Subscription of the app as current
     *  after the app posts it. */
    function appApi(items: PushSubscriptionDto[]) {
      let rows = items
      const api = stubApi(items)
      api.GET.mockImplementation(async (path: string) => {
        if (path === '/api/v1/push/key') return { data: { vapid_public_key: KEY } }
        if (path === '/api/v1/push-subscriptions') return { data: { items: rows } }
        throw new Error(`unexpected GET ${path}`)
      })
      api.POST.mockImplementation(async (path: string) => {
        if (path !== '/api/v1/push-subscriptions') throw new Error(`unexpected POST ${path}`)
        rows = [...rows.filter((row) => !row.current), THIS_APP]
        return { data: THIS_APP }
      })
      api.DELETE.mockImplementation(async (_path, init) => {
        steps.push('delete the Push Subscription')
        rows = rows.filter((row) => row.id !== init.params.path.push_subscription_id)
        return { response: new Response(null, { status: 204 }) }
      })
      return api
    }

    beforeEach(() => {
      steps.length = 0
      vi.stubGlobal('Capacitor', { isNativePlatform: () => true })
      vi.stubGlobal('PushManager', undefined)
      setNavigator('serviceWorker', {
        get ready() {
          ready()
          return new Promise(() => {})
        },
      })
      pagisPush.state.mockResolvedValue({ permission: 'prompt' })
      pagisPush.subscribe.mockResolvedValue(APP_SUBSCRIPTION)
      pagisPush.unsubscribe.mockImplementation(async () => {
        steps.push('unsubscribe')
      })
    })

    it('posts the answer of PagisPush.subscribe to the daemon', async () => {
      const api = appApi([PHONE])
      mount(api)

      await userEvent.click(await screen.findByRole('button', { name: 'Turn on' }))

      expect(pagisPush.subscribe).toHaveBeenCalledExactlyOnceWith({ vapidKey: KEY })
      await waitFor(() =>
        expect(api.POST).toHaveBeenCalledWith('/api/v1/push-subscriptions', {
          body: APP_SUBSCRIPTION,
        }),
      )
      expect(await screen.findByRole('button', { name: 'Turn off' })).toBeTruthy()
      expect(screen.getByText('This app gets notifications.')).toBeTruthy()
      expect(ready).not.toHaveBeenCalled()
      expect(pushManager.subscribe).not.toHaveBeenCalled()
      expect(pushManager.getSubscription).not.toHaveBeenCalled()
    })

    it('deletes the Push Subscription of the daemon, then unsubscribes the app', async () => {
      const api = appApi([PHONE, THIS_APP])
      mount(api)

      await userEvent.click(await screen.findByRole('button', { name: 'Turn off' }))

      await waitFor(() => expect(steps).toEqual(['delete the Push Subscription', 'unsubscribe']))
      expect(api.DELETE).toHaveBeenCalledExactlyOnceWith(
        '/api/v1/push-subscriptions/{push_subscription_id}',
        { params: { path: { push_subscription_id: 'ps-app' } } },
      )
      expect(await screen.findByRole('button', { name: 'Turn on' })).toBeTruthy()
      expect(ready).not.toHaveBeenCalled()
    })

    // A new Session has no Push Subscription. The app keeps its
    // registration with the Push Relay, and Turn on uses it again.
    it('shows Off when the daemon lists no Push Subscription of this Session', async () => {
      mount(appApi([PHONE]))

      expect(await screen.findByRole('button', { name: 'Turn on' })).toBeTruthy()
      expect(screen.getByText('This app does not get notifications.')).toBeTruthy()
      expect(pagisPush.unsubscribe).not.toHaveBeenCalled()
    })

    it('names the Push Subscription of the app so in the list', async () => {
      mount(appApi([PHONE, THIS_APP]))

      const rows = await screen.findAllByTestId('push-subscription-row')
      expect(within(rows[1]).getByText('This app')).toBeTruthy()
    })

    it('tells how to allow notifications when the phone blocks them', async () => {
      pagisPush.state.mockResolvedValue({ permission: 'denied' })
      mount(appApi([]))

      expect(await screen.findByText(/This phone blocks notifications from Pagis/)).toBeTruthy()
      expect(screen.queryByRole('button', { name: 'Turn on' })).toBeNull()
    })

    it('shows the words of the app when subscribe fails', async () => {
      pagisPush.subscribe.mockRejectedValue(new Error('The Push Relay did not answer.'))
      const api = appApi([])
      mount(api)

      await userEvent.click(await screen.findByRole('button', { name: 'Turn on' }))

      expect((await screen.findByRole('alert')).textContent).toBe('The Push Relay did not answer.')
      expect(api.POST).not.toHaveBeenCalled()
    })
  })

  it('removes one Push Subscription and reads the list again', async () => {
    const api = stubApi([PHONE])
    mount(api)

    await userEvent.click(await screen.findByRole('button', { name: 'Remove Pagis on iPhone' }))

    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/push-subscriptions/{push_subscription_id}', {
      params: { path: { push_subscription_id: 'ps-phone' } },
    })
    await waitFor(() => expect(screen.queryAllByTestId('push-subscription-row')).toHaveLength(0))
    expect(screen.getByText('No browser or app gets notifications.')).toBeTruthy()
  })
})
