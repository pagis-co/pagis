// The images of the page under test that load from another host.

/** The address of each image at a host other than the page's own. */
export function externalImages(): string[] {
  return [...document.querySelectorAll('img')]
    .map((image) => new URL(image.src, location.href))
    .filter((url) => url.protocol.startsWith('http') && url.origin !== location.origin)
    .map((url) => url.href)
}
