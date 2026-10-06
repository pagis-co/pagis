// The service worker of the Product App. It exists to receive push.
//
// It has no `fetch` listener and caches nothing: the Product App needs
// the daemon, and a stored page that shows a stale state is worse than
// an error. It holds no state, so a new worker takes control at once.

declare const self: ServiceWorkerGlobalScope

self.addEventListener('install', () => {
  void self.skipWaiting()
})

self.addEventListener('activate', (event) => {
  event.waitUntil(self.clients.claim())
})

// These listeners are empty: the worker shows no Notification.
self.addEventListener('push', () => {})

self.addEventListener('notificationclick', () => {})

export {}
