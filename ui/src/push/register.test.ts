import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { registerServiceWorker } from './register'

const register = vi.fn(() => Promise.resolve({} as ServiceWorkerRegistration))

function giveServiceWorker(): void {
  Object.defineProperty(navigator, 'serviceWorker', {
    configurable: true,
    value: { register },
  })
}

describe('registerServiceWorker', () => {
  beforeEach(() => {
    vi.stubEnv('PROD', true)
    giveServiceWorker()
  })

  afterEach(() => {
    vi.unstubAllEnvs()
    register.mockClear()
    Reflect.deleteProperty(navigator, 'serviceWorker')
    Reflect.deleteProperty(window, 'Capacitor')
  })

  it('registers the worker at the root of the origin in a production build', () => {
    registerServiceWorker()
    expect(register).toHaveBeenCalledExactlyOnceWith('/sw.js', { scope: '/' })
  })

  // The dev server builds no worker, so `/sw.js` is the entry page there.
  it('registers nothing in development', () => {
    vi.stubEnv('PROD', false)
    registerServiceWorker()
    expect(register).not.toHaveBeenCalled()
  })

  // A browser gives no service worker to an origin that is not secure,
  // such as `http://` on a LAN.
  it('registers nothing where the browser has no service worker', () => {
    Reflect.deleteProperty(navigator, 'serviceWorker')
    expect(() => registerServiceWorker()).not.toThrow()
    expect(register).not.toHaveBeenCalled()
  })

  // The Mobile App receives push through the native layer of the phone.
  it('registers nothing in the Mobile App', () => {
    Object.defineProperty(window, 'Capacitor', { configurable: true, value: {} })
    registerServiceWorker()
    expect(register).not.toHaveBeenCalled()
  })
})
