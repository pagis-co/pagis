// The service worker of the Product App (`sw/sw.ts`). It caches nothing;
// it exists to receive push.

/** Register `/sw.js` for the whole origin. Nothing happens in a
 * development build, where the dev server builds no worker; in a browser
 * with no service worker, as on an origin that is not secure; or in the
 * Mobile App, which receives push through the native layer of the phone
 * (WKWebView gives no service worker to a remote origin, and Android
 * System WebView has no Push API). */
export function registerServiceWorker(): void {
  if (!import.meta.env.PROD) return
  if (!('serviceWorker' in navigator)) return
  if ('Capacitor' in window) return
  navigator.serviceWorker.register('/sw.js', { scope: '/' }).catch((error: unknown) => {
    console.error('The service worker /sw.js did not register.', error)
  })
}
