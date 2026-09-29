import { describe, expect, it } from 'vitest'

import { isTrustedSetupRequest } from './setupTrust'

describe('setup IPC trust', () => {
  const setupUrl = 'file:///Applications/Pagis.app/Contents/Resources/app.asar/static/setup.html'
  const contents = { id: 7, getURL: () => setupUrl, mainFrame: { url: setupUrl } }

  it('accepts only the exact setup WebContents main frame', () => {
    expect(isTrustedSetupRequest({ sender: contents, senderFrame: contents.mainFrame }, contents, setupUrl)).toBe(true)
    expect(isTrustedSetupRequest({ sender: contents, senderFrame: null }, contents, setupUrl)).toBe(false)
  })

  it('rejects another WebContents, a subframe and a changed URL', () => {
    expect(isTrustedSetupRequest({ sender: { ...contents, id: 8 }, senderFrame: null }, contents, setupUrl)).toBe(false)
    expect(isTrustedSetupRequest({ sender: contents, senderFrame: { url: setupUrl } }, contents, setupUrl)).toBe(false)
    expect(isTrustedSetupRequest({ sender: contents, senderFrame: contents.mainFrame }, contents, `${setupUrl}?remote=1`)).toBe(false)
  })
})
