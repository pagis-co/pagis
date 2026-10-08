// The Notifications section (ADR-0030): the state of this browser, the
// turn-on and the turn-off, and every Push Subscription of the Person.
//
// In the Mobile App, the `PagisPush` plugin takes the place of
// `PushManager` (ADR-0032). The app is On when the daemon lists a Push
// Subscription of its Session. A new Session has none, so the section
// shows Off, and Turn on uses the registration with the Push Relay that
// the app holds.
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
import { Badge, Button, Frame, Row, SectionLabel, Switch } from '../../primitives'
import { PagisPush } from '../../push/pagisPush'
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
import { useIsMobile } from '../../state/useIsMobile'

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

/** The permission of the Mobile App to show notifications. */
const appPushKey = ['push', 'mobile-app'] as const

function useAppPush(enabled: boolean) {
  return useQuery({
    queryKey: appPushKey,
    queryFn: () => PagisPush.state(),
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

/** @param here The name of this client: "This browser" or "This app". */
function SubscriptionRow({
  api,
  row,
  here,
}: {
  api: ApiClient
  row: PushSubscriptionDto
  here: string
}) {
  const phone = useIsMobile()
  const test = useSendTestNotification(api)
  const remove = useRemovePushSubscription(api)
  const label = row.current ? here : sessionLabel(row)

  if (phone) return <Row data-testid="push-subscription-row"><span className="phone-row-copy"><strong>{label}</strong><span className="phone-hint">{row.last_sent_at == null ? 'nothing sent yet' : `last sent ${new Date(row.last_sent_at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`}</span></span><Button variant="link" className="phone-danger" aria-label={`Remove ${label}`} disabled={remove.isPending} onClick={() => remove.mutate(row.id)}>Remove</Button></Row>

  return (
    <Row className="push-row" data-testid="push-subscription-row">
      <span className="push-name">{label}</span>
      <span className="push-time">
        {row.last_sent_at == null
          ? 'nothing sent yet'
          : `last sent ${new Date(row.last_sent_at).toLocaleString()}`}
      </span>
      <span className="push-actions">
        {!phone && <Button
          size="sm"
          aria-label={`Send a test to ${label}`}
          disabled={test.isPending}
          onClick={() => test.mutate(row.id)}
        >
          Send a test
        </Button>}
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

function BlockedOnPhone() {
  return (
    <Note>
      This phone blocks notifications from Pagis. To allow them, open the Settings app of the
      phone, find Pagis, and turn on its notifications. Then open this page again.
    </Note>
  )
}

/** The state of this browser or app while it is read, or when a read
 *  failed. */
type View = PushState | 'reading' | 'unreadable'

export function useNotificationControl(api: ApiClient) {
  const support = pushSupport(window)
  const supported = support === 'supported'
  const mobileApp = support === 'mobile-app'
  const key = useVapidKey(api, supported || mobileApp)
  const list = usePushSubscriptions(api)
  const browser = useBrowserPush(supported)
  const app = useAppPush(mobileApp)
  const subscribe = useSubscribeToPush(api)
  const remove = useRemovePushSubscription(api)
  const queryClient = useQueryClient()
  const rows = list.data ?? []
  const held = browser.data?.held ?? null
  const listed = rows.some((row) => row.current)
  const here = mobileApp ? 'This app' : 'This browser'

  let view: View
  if (support === 'mobile-app') {
    if (key.isError || list.isError || app.isError) view = 'unreadable'
    else if (key.data === undefined || list.data === undefined || app.data === undefined)
      view = 'reading'
    else if (app.data.permission === 'denied') view = 'blocked'
    else view = listed ? 'on' : 'off'
  } else if (!supported) view = support
  else if (key.isError || list.isError || browser.isError) view = 'unreadable'
  else if (key.data === undefined || list.data === undefined || browser.data === undefined)
    view = 'reading'
  else view = pushState(window, held, { listed, key: key.data })

  const reread = () =>
    queryClient.invalidateQueries({ queryKey: mobileApp ? appPushKey : browserPushKey })

  // The browser waits on `subscribe()`, which the click started.
  const turnOn = useMutation({
    mutationFn: async (subscribing: Promise<components['schemas']['SubscribeRequest']>) => {
      await subscribe.mutateAsync(await subscribing)
    },
    onSettled: reread,
  })

  // The browser ends its subscription first. The app deletes the Push
  // Subscription of the daemon first, and then its registration with the
  // Push Relay.
  const turnOff = useMutation({
    mutationFn: async () => {
      if (!mobileApp) await held?.unsubscribe()
      for (const row of rows.filter((row) => row.current)) await remove.mutateAsync(row.id)
      if (mobileApp) await PagisPush.unsubscribe()
    },
    onSettled: reread,
  })

  useEffect(() => {
    if (view !== 'stale' || held === null) return
    held.unsubscribe().then(
      () => queryClient.invalidateQueries({ queryKey: browserPushKey }),
      (error: unknown) => console.error('The browser did not end its Push Subscription.', error),
    )
  }, [view, held, queryClient])

  const onTurnOn = () => {
    if (key.data === undefined) return
    if (mobileApp) {
      turnOff.reset()
      turnOn.mutate(PagisPush.subscribe({ vapidKey: key.data }))
      return
    }
    if (browser.data === undefined) return
    turnOff.reset()
    turnOn.mutate(
      browser.data.registration.pushManager
        .subscribe({
          userVisibleOnly: true,
          applicationServerKey: base64UrlBytes(key.data),
        })
        .then((subscription) => subscription.toJSON() as components['schemas']['SubscribeRequest']),
    )
  }
  const onTurnOff = () => {
    turnOn.reset()
    turnOff.mutate()
  }
  const busy = turnOn.isPending || turnOff.isPending
  const failed = turnOn.isError ? turnOn.error : turnOff.isError ? turnOff.error : null
  // The app rejects with words for the Person. A browser rejects with
  // the words of its own API, so there only a message of the daemon
  // shows.
  const failure =
    failed === null
      ? null
      : mobileApp && failed instanceof Error
        ? failed.message
        : errorMessage(failed, 'Notifications could not be changed in this browser.')

  return { view, here, mobileApp, list, rows, busy, failure, onTurnOn, onTurnOff }
}

export function NotificationSwitch({ api, hint = 'Tell me when something needs me.' }: { api: ApiClient; hint?: string }) {
  const { view, busy, failure, onTurnOn, onTurnOff } = useNotificationControl(api)
  return <><Switch row checked={view === 'on'} disabled={busy || !['on', 'off'].includes(view)} onCheckedChange={(on) => on ? onTurnOn() : onTurnOff()}><span className="phone-row-copy"><span>Notifications</span><span className="phone-hint">{view === 'blocked' ? 'Allow notifications in this phone’s Settings.' : view === 'add-to-home-screen' ? 'Add Pagis to your Home Screen to get notifications.' : view === 'not-available' ? 'Notifications are not available here.' : hint}</span></span></Switch>{failure && <p role="alert">{failure}</p>}</>
}

export function Notifications({ api }: { api: ApiClient }) {
  const { view, here, mobileApp, list, rows, busy, failure, onTurnOn, onTurnOff } = useNotificationControl(api)
  const phone = useIsMobile()
  return (
    <SettingsSection
      title="Notifications"
      lead="The browsers and apps that tell you when something needs you."
      hint={phone ? "Pagis holds a notification back while you use Pagis on any device." : (
        <>
          A notification comes when something needs you, also when no Pagis window is open.{' '}
          <a href={NOTIFICATIONS_GUIDE} target="_blank" rel="noreferrer">
            Notifications
          </a>
        </>
      )}
    >
      {phone && <SectionLabel>This phone</SectionLabel>}
      <Frame>
        {phone && <NotificationSwitch api={api} hint={`${here} ${view === 'on' ? 'gets' : 'does not get'} notifications.`} />}
        {!phone && <>{view === 'not-available' && (
          <Note>
            This app does not show notifications. Turn them on in a browser on your phone or your
            computer.
          </Note>
        )}
        {view === 'add-to-home-screen' && <AddToHomeScreen />}
        {view === 'blocked' && (mobileApp ? <BlockedOnPhone /> : <Blocked />)}
        {view === 'reading' && <Note>Reading…</Note>}
        {view === 'unreadable' && (
          <Note>The notifications of {here.toLowerCase()} could not be read.</Note>
        )}
        {(view === 'off' || view === 'stale') && (
          <Row className="push-row">
            <Badge tone="neutral">Off</Badge>
            <span className="push-note">{here} does not get notifications.</span>
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
            <span className="push-note">{here} gets notifications.</span>
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
        {failure !== null && (
          <Row>
            <span className="settings-error" role="alert">
              {failure}
            </span>
          </Row>
        )}
        </>}
      </Frame>
      {phone && <SectionLabel>Other browsers and apps</SectionLabel>}
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
          rows.filter((row) => !phone || !row.current).map((row) => <SubscriptionRow key={row.id} api={api} row={row} here={here} />)
        )}
      </Frame>
    </SettingsSection>
  )
}
