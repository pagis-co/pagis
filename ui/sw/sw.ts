// The service worker of the Product App. It exists to receive push: it
// shows the Notification of each push, opens its place on a tap, and
// posts the answer of an action button. The logic is in `handlers.ts`.
//
// It has no `fetch` listener and caches nothing: the Product App needs
// the daemon, and a stored page that shows a stale state is worse than
// an error. It holds no state, so a new worker takes control at once.

import { clickNotification, type NotificationClass, showPush } from './handlers'

// A worker of a browser with no Notifications API has no `Notification`.
declare const self: ServiceWorkerGlobalScope & { Notification?: NotificationClass }

self.addEventListener('install', () => {
  void self.skipWaiting()
})

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim())
})

self.addEventListener('push', (event) => {
  event.waitUntil(
    showPush(self.registration, self.navigator, self.Notification ?? {}, event.data?.text()),
  )
})

self.addEventListener('notificationclick', (event) => {
  event.waitUntil(
    clickNotification(event.action, event.notification, {
      clients: self.clients,
      origin: self.location.origin,
      registration: self.registration,
      // `fetch` throws when it is not called on the scope.
      fetch: (url, init) => self.fetch(url, init),
    }),
  )
})
