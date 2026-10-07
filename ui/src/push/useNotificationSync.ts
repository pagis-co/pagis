// The open page keeps the Notifications and the app badge on the
// daemon's Needs-You Queue (ADR-0030). The daemon sends no push when an
// item leaves the queue, so the page closes the Notifications of the
// items that left, and sets the badge to the count. Only the service
// worker shows a Notification (`sw/handlers.ts`); the page shows none.

import { useNavigate } from '@tanstack/react-router'
import { useEffect, useState } from 'react'

import type { ApiClient } from '../api/client'
import { useNeedsYou } from '../queries'

/** The URL of a `navigate` message of the service worker, or `null` for
 * every other message. */
function navigateUrl(message: unknown): URL | null {
  if (typeof message !== 'object' || message === null) return null
  const { type, url } = message as { type?: unknown; url?: unknown }
  if (type !== 'navigate' || typeof url !== 'string') return null
  try {
    return new URL(url, window.location.origin)
  } catch {
    return null
  }
}

/** Set the app badge to `count`, and clear it at 0, where the browser
 * has the Badging API. */
function setBadge(count: number): void {
  if (!('setAppBadge' in navigator)) return
  const change = count === 0 ? navigator.clearAppBadge() : navigator.setAppBadge(count)
  change.catch((error: unknown) => {
    console.error(`The app badge did not change to ${count}.`, error)
  })
}

/** Keep the Notifications and the app badge on the Needs-You Queue, and
 * move the router to the place that the service worker names after a
 * tap on a Notification. With no service worker registration, as in a
 * development build or in the Mobile App, it does nothing. */
export function useNotificationSync(api: ApiClient): void {
  const navigate = useNavigate()
  const queue = useNeedsYou(api).data
  const [registration, setRegistration] = useState<ServiceWorkerRegistration | null>(null)

  useEffect(() => {
    if (!('serviceWorker' in navigator)) return
    let mounted = true
    navigator.serviceWorker.getRegistration().then(
      (found) => {
        if (mounted && found !== undefined) setRegistration(found)
      },
      (error: unknown) => {
        console.error('The service worker registration did not load.', error)
      },
    )
    return () => {
      mounted = false
    }
  }, [])

  useEffect(() => {
    if (registration === null) return
    const container = navigator.serviceWorker
    const onMessage = (event: MessageEvent) => {
      const url = navigateUrl(event.data)
      if (url !== null) void navigate({ href: `${url.pathname}${url.search}${url.hash}` })
    }
    container.addEventListener('message', onMessage)
    return () => container.removeEventListener('message', onMessage)
  }, [registration, navigate])

  useEffect(() => {
    if (registration === null || queue === undefined) return
    // A newer queue replaces this one before the list arrives, so a
    // Notification of an item that entered since then stays open.
    let current = true
    const itemIds = new Set(queue.items.map((item) => item.id))
    registration.getNotifications().then(
      (notifications) => {
        if (!current) return
        for (const notification of notifications) {
          if (!itemIds.has(notification.tag)) notification.close()
        }
      },
      (error: unknown) => {
        console.error('The shown Notifications did not load.', error)
      },
    )
    setBadge(queue.count)
    return () => {
      current = false
    }
  }, [registration, queue])
}
