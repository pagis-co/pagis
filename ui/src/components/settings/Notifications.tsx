// The Notifications section (ADR-0030): the state of this browser, the
// turn-on and the turn-off, and every Push Subscription of the Person.
//
// The key loads when the section opens. A click on Turn on calls
// `subscribe()` before it returns, with no network wait: a wait inside
// the click can end the user gesture in Safari, and then `subscribe()`
// fails. `subscribe()` asks for the notification permission itself.
//
// The daemon's list is the truth. A Push Subscription that this browser
// holds and that the daemon does not list, or that has another key, is
// ended, and the section shows Off.

import { useEffect, type ReactNode } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import type { ApiClient, PushOutcomeDto, PushSubscriptionDto } from '../../api/client'
import type { components } from '../../api/schema'
import { Badge, Button, Frame, Row } from '../../primitives'
import { base64UrlBytes, pushState, pushSupport, type PushState } from '../../push/support'
import {
  errorMessage,
  usePushSubscriptions,
  useRemovePushSubscription,
  useSendTestNotification,
  useSubscribeToPush,
  useVapidKey,
} from '../../queries'
import { sessionLabel } from './Sessions'
import { SettingsSection } from './SettingsSection'

import './Notifications.css'

/** The documentation page of Notifications. */
export const NOTIFICATIONS_GUIDE = 'https://docs.pagis.co/notifications'

/** The service worker registration of this page and the Push
 *  Subscription that it holds. */
const browserPushKey = ['push', 'browser'] as const

function useBrowserPush(enabled: boolean) {
  return useQuery({
    queryKey: browserPushKey,
    queryFn: async () => {
      const registration = await navigator.serviceWorker.ready
      return { registration, held: await registration.pushManager.getSubscription() }
    },
    enabled,
  })
}

/** What the push service answered to a test, in the Person's words. */
function outcomeText(outcome: PushOutcomeDto): string {
  switch (outcome.outcome) {
    case 'delivered':
      return 'Sent.'
    case 'gone':
      return 'The push service ended this subscription, so Pagis removed it.'
    case 'too_large':
      return 'The push service refused the size of the notification.'
    case 'rate_limited':
      return outcome.retry_after_seconds
        ? `The push service asks Pagis to wait ${outcome.retry_after_seconds} seconds.`
        : 'The push service asks Pagis to wait. Try again later.'
    case 'failed':
      return `The push service did not take the notification: ${outcome.error}`
  }
}

function SubscriptionRow({ api, row }: { api: ApiClient; row: PushSubscriptionDto }) {
  const test = useSendTestNotification(api)
  const remove = useRemovePushSubscription(api)
  const label = row.current ? 'This browser' : sessionLabel(row)

  return (
    <Row className="push-row" data-testid="push-subscription-row">
      <span className="push-name">{label}</span>
      <span className="push-time">
        {row.last_sent_at == null
          ? 'nothing sent yet'
          : `last sent ${new Date(row.last_sent_at).toLocaleString()}`}
      </span>
      <span className="push-actions">
        <Button
          size="sm"
          aria-label={`Send a test to ${label}`}
          disabled={test.isPending}
          onClick={() => test.mutate(row.id)}
        >
          Send a test
        </Button>
        <Button
          size="sm"
          variant="danger-quiet"
          aria-label={`Remove ${label}`}
          disabled={remove.isPending}
          onClick={() => remove.mutate(row.id)}
        >
          Remove
        </Button>
      </span>
      {test.isSuccess && <span className="push-note">{outcomeText(test.data)}</span>}
      {test.isError && (
        <span className="settings-error" role="alert">
          {errorMessage(test.error, 'The test could not be sent.')}
        </span>
      )}
      {remove.isError && (
        <span className="settings-error" role="alert">
          {errorMessage(remove.error, 'That subscription could not be removed.')}
        </span>
      )}
    </Row>
  )
}

/** The words of a state that has no control. */
function Note({ children }: { children: ReactNode }) {
  return (
    <Row>
      <span className="push-note">{children}</span>
    </Row>
  )
}

function AddToHomeScreen() {
  return (
    <Row className="push-steps">
      <span className="push-note">
        On an iPhone or an iPad, only Pagis on the Home Screen gets notifications.
      </span>
      <ol className="push-step-list">
        <li>In Safari, select Share.</li>
        <li>
          Select <strong>Add to Home Screen</strong>.
        </li>
      </ol>
      <span className="push-note">
        Open Pagis from the Home Screen and turn on notifications here.
      </span>
    </Row>
  )
}

function Blocked() {
  return (
    <Note>
      This browser blocks notifications from this site. To allow them, select the icon at the left
      of the address bar and set Notifications to Allow. Then open this page again.{' '}
      <a
        href={`${NOTIFICATIONS_GUIDE}#allow-notifications-that-the-browser-blocks`}
        target="_blank"
        rel="noreferrer"
      >
        Allow notifications
      </a>
    </Note>
  )
}

/** The state of this browser while it is read, or when a read failed. */
type View = PushState | 'reading' | 'unreadable'

export function Notifications({ api }: { api: ApiClient }) {
  const support = pushSupport(window)
  const supported = support === 'supported'
  const key = useVapidKey(api, supported)
  const list = usePushSubscriptions(api)
  const browser = useBrowserPush(supported)
  const subscribe = useSubscribeToPush(api)
  const remove = useRemovePushSubscription(api)
  const queryClient = useQueryClient()
  const rows = list.data ?? []
  const held = browser.data?.held ?? null

  let view: View
  if (!supported) view = support
  else if (key.isError || list.isError || browser.isError) view = 'unreadable'
  else if (key.data === undefined || list.data === undefined || browser.data === undefined)
    view = 'reading'
  else view = pushState(window, held, { listed: rows.some((row) => row.current), key: key.data })

  const rereadBrowser = () => queryClient.invalidateQueries({ queryKey: browserPushKey })

  // The browser waits on `subscribe()`, which the click started.
  const turnOn = useMutation({
    mutationFn: async (subscribing: Promise<PushSubscription>) => {
      const subscription = await subscribing
      await subscribe.mutateAsync(
        subscription.toJSON() as components['schemas']['SubscribeRequest'],
      )
    },
    onSettled: rereadBrowser,
  })

  const turnOff = useMutation({
    mutationFn: async () => {
      await held?.unsubscribe()
      for (const row of rows.filter((row) => row.current)) await remove.mutateAsync(row.id)
    },
    onSettled: rereadBrowser,
  })

  useEffect(() => {
    if (view !== 'stale' || held === null) return
    held.unsubscribe().then(
      () => queryClient.invalidateQueries({ queryKey: browserPushKey }),
      (error: unknown) => console.error('The browser did not end its Push Subscription.', error),
    )
  }, [view, held, queryClient])

  const onTurnOn = () => {
    if (browser.data === undefined || key.data === undefined) return
    turnOff.reset()
    turnOn.mutate(
      browser.data.registration.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: base64UrlBytes(key.data),
      }),
    )
  }
  const onTurnOff = () => {
    turnOn.reset()
    turnOff.mutate()
  }
  const busy = turnOn.isPending || turnOff.isPending
  const failed = turnOn.isError ? turnOn.error : turnOff.isError ? turnOff.error : null

  return (
    <SettingsSection
      title="Notifications"
      lead="The browsers and apps that tell you when something needs you."
      hint={
        <>
          A notification comes when something needs you, also when no Pagis window is open.{' '}
          <a href={NOTIFICATIONS_GUIDE} target="_blank" rel="noreferrer">
            Notifications
          </a>
        </>
      }
    >
      {view !== 'mobile-app' && (
        <Frame>
          {view === 'not-available' && (
            <Note>
              This app does not show notifications. Turn them on in a browser on your phone or your
              computer.
            </Note>
          )}
          {view === 'add-to-home-screen' && <AddToHomeScreen />}
          {view === 'blocked' && <Blocked />}
          {view === 'reading' && <Note>Reading…</Note>}
          {view === 'unreadable' && <Note>The notifications of this browser could not be read.</Note>}
          {(view === 'off' || view === 'stale') && (
            <Row className="push-row">
              <Badge tone="neutral">Off</Badge>
              <span className="push-note">This browser does not get notifications.</span>
              <Button
                variant="primary"
                className="push-actions"
                // A stale subscription ends first.
                disabled={busy || view === 'stale'}
                onClick={onTurnOn}
              >
                Turn on
              </Button>
            </Row>
          )}
          {view === 'on' && (
            <Row className="push-row">
              <Badge tone="accent">On</Badge>
              <span className="push-note">This browser gets notifications.</span>
              <Button
                variant="danger-quiet"
                className="push-actions"
                disabled={busy}
                onClick={onTurnOff}
              >
                Turn off
              </Button>
            </Row>
          )}
          {failed !== null && (
            <Row>
              <span className="settings-error" role="alert">
                {errorMessage(failed, 'Notifications could not be changed in this browser.')}
              </span>
            </Row>
          )}
        </Frame>
      )}
      <Frame>
        {rows.length === 0 ? (
          <Note>
            {list.isError
              ? 'The list of notifications could not be read.'
              : list.isPending
                ? 'Reading…'
                : 'No browser or app gets notifications.'}
          </Note>
        ) : (
          rows.map((row) => <SubscriptionRow key={row.id} api={api} row={row} />)
        )}
      </Frame>
    </SettingsSection>
  )
}
