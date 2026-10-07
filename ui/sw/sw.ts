// The service worker of the Product App. It exists to receive push: it
// shows the Notification of each push and opens its place on a tap. The
// logic is in `handlers.ts`.
//
// It has no `fetch` listener and caches nothing: the Product App needs
// the daemon, and a stored page that shows a stale state is worse than
// an error. It holds no state, so a new worker takes control at once.

import { openPlace, showPush } from './handlers'

declare const self: ServiceWorkerGlobalScope

self.addEventListener('install', () => {
  void self.skipWaiting()
})

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim())
})

self.addEventListener('push', (event) => {
  event.waitUntil(showPush(self.registration, self.navigator, event.data?.text()))
})

self.addEventListener('notificationclick', (event) => {
  event.waitUntil(openPlace(event.notification, self.clients, self.location.origin))
})
