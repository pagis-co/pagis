import { beforeEach, expect, it, vi } from 'vitest'
const mocks = vi.hoisted(() => ({ expose: vi.fn(), invoke: vi.fn() }))
vi.mock('electron', () => ({ contextBridge: { exposeInMainWorld: mocks.expose }, ipcRenderer: { invoke: mocks.invoke } }))
beforeEach(() => { vi.resetModules(); mocks.expose.mockClear(); mocks.invoke.mockClear() })

it('exposes only the one-use sign-in verifier in the main frame', async () => {
  Object.assign(process, { isMainFrame: true })
  await import('./productPreload')
  const [name, api] = mocks.expose.mock.calls[0]
  expect(name).toBe('pagisDesktop')
  expect(Object.keys(api)).toEqual(['takeSignInVerifier'])
  await api.takeSignInVerifier('secret')
  expect(mocks.invoke).toHaveBeenCalledWith('pagis:sign-in-verifier', 'secret')
})
it('exposes no bridge in a sub-frame', async () => {
  Object.assign(process, { isMainFrame: false })
  await import('./productPreload')
  expect(mocks.expose).not.toHaveBeenCalled()
})
