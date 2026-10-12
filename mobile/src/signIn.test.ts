import { webcrypto } from 'node:crypto'
import { describe, expect, it, vi } from 'vitest'
import { openConnectedServer } from './signIn'

const origin = 'https://pagis.example/'

function shell(callback: (url: URL) => string) {
  return {
    authenticate: vi.fn(async ({ url }: { url: string }) => ({ url: callback(new URL(url)) })),
    open: vi.fn(async (_address: { origin: string; opens: string; verifier?: string }) => {}),
  }
}

describe('system browser sign-in', () => {
  it('refuses a callback with the wrong state', async () => {
    const native = shell(() => 'pagis-app://signed-in?state=wrong#secret')
    await expect(openConnectedServer({ origin, opens: origin }, 'ios', native, webcrypto as Crypto))
      .rejects.toThrow(/state/)
    expect(native.open).not.toHaveBeenCalled()
  })

  it.each(['ios', 'android'])('opens the web view with the secret and verifier on %s', async (platform) => {
    const native = shell((url) => `pagis-app://signed-in?state=${url.searchParams.get('state')}#secret`)
    await openConnectedServer({ origin, opens: origin }, platform, native, webcrypto as Crypto)
    const request = new URL(native.authenticate.mock.calls[0][0].url)
    expect(request.origin).toBe('https://pagis.example')
    expect(request.pathname).toBe('/sign-in')
    expect(request.searchParams.get('app')).toBe(platform)
    const address = native.open.mock.calls[0][0] as { opens: string; verifier: string }
    expect(address).toEqual({ origin, opens: `${origin}sign-in#secret`, verifier: expect.any(String) })
    const digest = await webcrypto.subtle.digest('SHA-256', new TextEncoder().encode(address.verifier))
    expect(request.searchParams.get('challenge')).toBe(Buffer.from(digest).toString('base64url'))
    expect(address.verifier.length).toBe(43)
    expect(request.searchParams.get('state')?.length).toBe(43)
  })

  it.each(['https://evil.example/?state=x#secret', 'pagis-app://other?state=x#secret', 'pagis-app://signed-in?state=x'])('refuses an invalid callback %s', async (callback) => {
    const native = shell(() => callback)
    await expect(openConnectedServer({ origin, opens: origin }, 'ios', native, webcrypto as Crypto)).rejects.toThrow()
    expect(native.open).not.toHaveBeenCalled()
  })

  it('opens a scanned or pasted link directly', async () => {
    const native = shell(() => '')
    const address = { origin, opens: `${origin}sign-in#pasted` }
    await openConnectedServer(address, 'ios', native, webcrypto as Crypto)
    expect(native.authenticate).not.toHaveBeenCalled()
    expect(native.open).toHaveBeenCalledWith(address)
  })
})
