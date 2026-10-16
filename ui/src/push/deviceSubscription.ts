/** The server record and Public Origin of this device's Push Subscription. */
export interface DeviceSubscription {
  origin: string | null
  id: string | null
}

const KEY = 'pagis.push-subscription'

/** Browser storage belongs to its origin. A new origin has no subscription. */
export function browserSubscription(): DeviceSubscription | null {
  try {
    const stored: unknown = JSON.parse(window.localStorage.getItem(KEY) ?? 'null')
    if (stored === null || typeof stored !== 'object') return null
    const { origin, id } = stored as Record<string, unknown>
    return typeof origin === 'string' && typeof id === 'string' ? { origin, id } : null
  } catch {
    return null
  }
}

export function keepBrowserSubscription(id: string): void {
  window.localStorage.setItem(KEY, JSON.stringify({ origin: window.location.origin, id }))
}
