import { afterEach, expect, it, vi } from 'vitest'
import { takeSignInVerifier } from './mobileShell'
afterEach(() => { delete window.pagisDesktop })
it('takes the Client App verifier for the secret without a Mobile App bridge', async () => {
  const take = vi.fn(async () => 'desktop-verifier')
  window.pagisDesktop = { takeSignInVerifier: take }
  expect(await takeSignInVerifier('bound-secret')).toBe('desktop-verifier')
  expect(take).toHaveBeenCalledWith('bound-secret')
})
it('has no verifier in an ordinary browser', async () => {
  expect(await takeSignInVerifier('secret')).toBeUndefined()
})
