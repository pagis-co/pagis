// The logic of the service worker of the Product App (ADR-0030). Each
// function takes the parts of the browser that it uses as arguments, so
// `sw.ts` only connects them to the events.

/** The image of the Notification, at the root of the Product App. */
const ICON = '/icon-192.png'

/** The monochrome image that Android shows in the status bar. */
const BADGE = '/badge-96.png'

/** The version of the payload format that this worker reads. */
const PAYLOAD_VERSION = 1

/** One button of a Notification. The DOM types do not hold it, because
 * Safari shows none. */
interface NotificationAction {
  action: string
  title: string
}

/** The options of `showNotification`. `navigate` is the member of the
 * WebKit Declarative Web Push format, and `actions` the buttons that
 * Chrome, Edge and Firefox show. The DOM types hold neither. */
export type PushNotificationOptions = NotificationOptions & {
  navigate: string
  actions?: NotificationAction[]
}

/** The static members of `Notification` that the worker reads. Safari
 * has no `maxActions`, and shows no action. */
export interface NotificationClass {
  readonly maxActions?: number
}

/** The buttons of a Notification that answers a Request, each with the
 * decision that it posts. A Notification approves once only: an Allow
 * Rule needs the approval card, which states what the rule covers. */
const ANSWERS = [
  { action: 'approve_once', title: 'Approve once', decision: 'approved' },
  { action: 'deny', title: 'Deny', decision: 'denied' },
] as const

/** How long the worker waits for the daemon to take an answer. */
const ANSWER_TIMEOUT_MS = 20_000

/** The text of the Notification that replaces one whose answer did not
 * go through. */
const ANSWER_FAILED = 'Pagis did not take this answer. Open Pagis to see the request.'

/** What the worker shows for one push, and the app badge it sets. */
interface Shown {
  title: string
  options: PushNotificationOptions
  appBadge: number | undefined
  /** Whether the Notification can answer a Request. */
  answers: boolean
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
  answers: false,
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
    answers: isRecord(data.request),
  }
}

/** Show the Notification of one push, and set the app badge where the
 * payload has one and the browser has the Badging API. Every push shows
 * a Notification, because a browser can end a subscription whose push
 * shows none. A Notification that answers a Request shows **Approve
 * once** and **Deny** where the browser shows two actions. */
export async function showPush(
  registration: Pick<ServiceWorkerRegistration, 'showNotification'>,
  navigator: Partial<Pick<WorkerNavigator, 'setAppBadge'>>,
  notificationClass: NotificationClass,
  text: string | undefined,
): Promise<void> {
  const shown = read(text)
  const options =
    shown.answers && (notificationClass.maxActions ?? 0) >= ANSWERS.length
      ? { ...shown.options, actions: ANSWERS.map(({ action, title }) => ({ action, title })) }
      : shown.options
  await registration.showNotification(shown.title, options)
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

/** The parts of `fetch` that post an answer. */
type Fetch = (url: string, init: RequestInit) => Promise<Pick<Response, 'ok' | 'status'>>

/** The parts of the worker that a click on a Notification uses. */
export interface ClickContext {
  clients: WindowClients
  origin: string
  registration: Pick<ServiceWorkerRegistration, 'showNotification'>
  fetch: Fetch
}

/** The Notification that the Person clicked. */
type ClickedNotification = Pick<Notification, 'close' | 'data' | 'tag' | 'title'>

/** Act on a click on a Notification. A click on an action button posts
 * its answer to the Request. A click on the body, where `action` is
 * empty, opens the place of the item. */
export async function clickNotification(
  action: string,
  notification: ClickedNotification,
  worker: ClickContext,
): Promise<void> {
  const answer = ANSWERS.find((answer) => answer.action === action)
  const requestId = requestIdOf(notification.data)
  if (answer === undefined || requestId === undefined) {
    await openPlace(notification, worker.clients, worker.origin)
    return
  }
  if (await postDecision(requestId, answer.decision, worker.fetch)) {
    notification.close()
    return
  }
  // The same tag replaces the Notification. Its body opens the place of
  // the item, where the approval card shows the state of the Request.
  const place = isRecord(notification.data) ? notification.data.navigate : undefined
  const navigate = typeof place === 'string' ? place : '/'
  const failure: PushNotificationOptions = {
    body: ANSWER_FAILED,
    tag: notification.tag,
    data: { navigate },
    navigate,
    icon: ICON,
    badge: BADGE,
  }
  await worker.registration.showNotification(notification.title, failure)
}

/** The id of the Request that a Notification answers, if it answers one. */
function requestIdOf(data: unknown): string | undefined {
  if (!isRecord(data) || !isRecord(data.request)) return undefined
  return typeof data.request.id === 'string' ? data.request.id : undefined
}

/** Post a decision with the Session cookie of the browser, and answer
 * whether the daemon took it. The worker keeps no record of the answer:
 * the daemon records the decision, and its `needs_you.removed` event
 * clears the item on every client. */
async function postDecision(requestId: string, decision: string, fetch: Fetch): Promise<boolean> {
  const timeout = new AbortController()
  const timer = setTimeout(() => timeout.abort(), ANSWER_TIMEOUT_MS)
  try {
    const response = await fetch(`/api/v1/requests/${encodeURIComponent(requestId)}/decision`, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ decision }),
      signal: timeout.signal,
    })
    if (response.ok) return true
    console.error(`The daemon answered ${response.status} to the decision on request ${requestId}.`)
  } catch (error) {
    console.error(`The decision on request ${requestId} did not reach the daemon.`, error)
  } finally {
    clearTimeout(timer)
  }
  return false
}
