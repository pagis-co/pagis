// The logic of the service worker of the Product App (ADR-0030). Each
// function takes the parts of the browser that it uses as arguments, so
// `sw.ts` only connects them to the events.

/** The image of the Notification, at the root of the Product App. */
const ICON = '/icon-192.png'

/** The monochrome image that Android shows in the status bar. */
const BADGE = '/badge-96.png'

/** The version of the payload format that this worker reads. */
const PAYLOAD_VERSION = 1

/** The options of `showNotification`. `navigate` is the member of the
 * WebKit Declarative Web Push format, which the DOM types do not hold. */
export type PushNotificationOptions = NotificationOptions & { navigate: string }

/** What the worker shows for one push, and the app badge it sets. */
interface Shown {
  title: string
  options: PushNotificationOptions
  appBadge: number | undefined
}

/** The Notification of a payload that this worker cannot read: an
 * unknown version, a payload that does not parse, or no payload. A tap
 * on it opens the Product App. */
const PLACEHOLDER: Shown = {
  title: 'Pagis',
  options: {
    body: 'Something needs you',
    data: { navigate: '/' },
    navigate: '/',
    icon: ICON,
    badge: BADGE,
  },
  appBadge: undefined,
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

/** Read the Declarative Web Push JSON that the daemon writes. The Pagis
 * fields are in `notification.data`, and the app badge is at the top
 * level of the message. */
function read(text: string | undefined): Shown {
  if (text === undefined) return PLACEHOLDER
  let message: unknown
  try {
    message = JSON.parse(text)
  } catch {
    return PLACEHOLDER
  }
  if (!isRecord(message) || !isRecord(message.notification)) return PLACEHOLDER
  const { title, body, navigate, data } = message.notification
  if (typeof title !== 'string' || !isRecord(data)) return PLACEHOLDER
  if (data.v !== PAYLOAD_VERSION || typeof data.item !== 'string') return PLACEHOLDER
  const place = typeof navigate === 'string' ? navigate : '/'
  return {
    title,
    options: {
      body: typeof body === 'string' ? body : '',
      // A second push for one item replaces the first.
      tag: data.item,
      data: { ...data, navigate: place },
      navigate: place,
      icon: ICON,
      badge: BADGE,
    },
    appBadge: typeof message.app_badge === 'number' ? message.app_badge : undefined,
  }
}

/** Show the Notification of one push, and set the app badge where the
 * payload has one and the browser has the Badging API. Every push shows
 * a Notification, because a browser can end a subscription whose push
 * shows none. */
export async function showPush(
  registration: Pick<ServiceWorkerRegistration, 'showNotification'>,
  navigator: Partial<Pick<WorkerNavigator, 'setAppBadge'>>,
  text: string | undefined,
): Promise<void> {
  const shown = read(text)
  await registration.showNotification(shown.title, shown.options)
  if (shown.appBadge === undefined || navigator.setAppBadge === undefined) return
  try {
    await navigator.setAppBadge(shown.appBadge)
  } catch (error) {
    // The Notification shows. Only the count on the icon is stale until
    // the Product App opens.
    console.error(`The app badge did not change to ${shown.appBadge}.`, error)
  }
}

/** The parts of `Clients` that open a place. */
interface WindowClients {
  matchAll(options: { type: 'window'; includeUncontrolled: true }): Promise<
    readonly { postMessage(message: unknown): void; focus(): Promise<unknown> }[]
  >
  openWindow(url: string): Promise<unknown>
}

/** The place of a Notification as a URL on `origin`. A place on another
 * origin, or no place, gives the root of the Product App. */
function placeOf(data: unknown, origin: string): string {
  const root = new URL('/', origin).href
  if (!isRecord(data) || typeof data.navigate !== 'string') return root
  try {
    const place = new URL(data.navigate, origin)
    return place.origin === origin ? place.href : root
  } catch {
    return root
  }
}

/** Open the place of a Notification that the Person tapped. An open
 * window gets the focus and a `navigate` message, and moves its own
 * router: `WindowClient.navigate()` loads the page again and loses its
 * state. With no open window, a new window opens at the place. */
export async function openPlace(
  notification: Pick<Notification, 'close' | 'data'>,
  clients: WindowClients,
  origin: string,
): Promise<void> {
  notification.close()
  const url = placeOf(notification.data, origin)
  const [window] = await clients.matchAll({ type: 'window', includeUncontrolled: true })
  if (window === undefined) {
    await clients.openWindow(url)
    return
  }
  window.postMessage({ type: 'navigate', url })
  await window.focus()
}
