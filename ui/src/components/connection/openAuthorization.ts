// The browser step of a Google Connection (ADR-0012).

/** Open the start route that the authorize request answered. The
 *  person finishes at Google in their own browser, on whatever machine
 *  they are on, so the client opens a tab and the record catches up.
 *  The start route sends a browser of this Person on to Google, and the
 *  Client App opens it in the system browser. A Connection that the
 *  request finished answers no address, and nothing opens. */
export function openAuthorization(url: string | null | undefined) {
  if (url != null && url !== '') window.open(url, '_blank', 'noopener')
}
