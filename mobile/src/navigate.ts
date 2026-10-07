/**
 * The rule of the tap on a Notification (ADR-0032). A Notification holds
 * `navigate`, the absolute URL of the place of its item on the Public
 * Origin. The app opens that place only when its origin is the stored
 * origin of the server, and then it opens the path, the query and the
 * fragment of the URL in the web view. Each other value opens the root of
 * the Product App, so a Notification never takes the web view to another
 * origin.
 *
 * The native shell applies the rule: `WebOrigin.place(of:)` on iOS and
 * `ServerOrigin.place` on Android. Their tests take the cases of
 * `navigate.test.ts`.
 */

/** The place that a tap opens on the server at `origin`: a path that
 *  starts with `/`. */
export function placeOf(navigate: unknown, origin: string): string {
  if (typeof navigate !== 'string') return '/'
  let url: URL
  try {
    url = new URL(navigate)
  } catch {
    return '/'
  }
  if (url.origin !== new URL(origin).origin) return '/'
  return `${url.pathname}${url.search}${url.hash}`
}
