/* jsdom does not implement the pointer-capture, layout and observer
 * calls that the Radix overlays make. Each stub below stands in for one
 * of them, so a keyboard test runs against the real primitive. */

if (!('PointerEvent' in globalThis)) {
  // @ts-expect-error jsdom has no PointerEvent; MouseEvent carries the
  // fields the primitives read.
  globalThis.PointerEvent = MouseEvent
}

if (globalThis.Element !== undefined) {
  const element = globalThis.Element.prototype as unknown as Record<string, unknown>
  element.hasPointerCapture ??= () => false
  element.setPointerCapture ??= () => undefined
  element.releasePointerCapture ??= () => undefined
  element.scrollIntoView ??= () => undefined
}

// jsdom has no media queries. The default is the wide screen; a test
// that wants the phone stubs `window.matchMedia` itself.
if (globalThis.window !== undefined && window.matchMedia === undefined) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia
}

if (!('ResizeObserver' in globalThis)) {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver
}

// Node runs jsdom without a storage area unless it starts with
// `--localstorage-file`. The theme keeps its choice here.
if (globalThis.window !== undefined && window.localStorage === undefined) {
  const entries = new Map<string, string>()
  Object.defineProperty(window, 'localStorage', {
    configurable: true,
    value: {
      get length() {
        return entries.size
      },
      key: (index: number) => [...entries.keys()][index] ?? null,
      getItem: (key: string) => entries.get(key) ?? null,
      setItem: (key: string, value: string) => void entries.set(key, String(value)),
      removeItem: (key: string) => void entries.delete(key),
      clear: () => entries.clear(),
    } satisfies Storage,
  })
}

if (!('DOMRect' in globalThis)) {
  globalThis.DOMRect = class {
    constructor(
      public x = 0,
      public y = 0,
      public width = 0,
      public height = 0,
    ) {}
    top = 0
    left = 0
    right = 0
    bottom = 0
    toJSON() {
      return this
    }
    static fromRect() {
      return new globalThis.DOMRect()
    }
  } as unknown as typeof DOMRect
}
