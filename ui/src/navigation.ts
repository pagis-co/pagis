// A load of a document that is not a view of the Product App, such as a
// route of the daemon that answers a redirect. The router moves between
// views inside the page; this leaves the page.

/** Load `address` in place of the page. The address replaces the page
 *  in the history, so Back does not open the page again. */
export function openDocument(address: string): void {
  window.location.replace(address)
}
