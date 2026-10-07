import { describe, expect, it, vi } from 'vitest'

import { scanQrCode, type BarcodeScanner } from './scan'

/** A scanner that answers as `@capacitor/barcode-scanner` does. */
function scanner(answer: () => Promise<{ ScanResult: string }>): BarcodeScanner {
  return vi.fn(async () => ({ ...(await answer()), format: 0 }))
}

/** The plugin rejects with a code `OS-PLUG-BARC-<number>`. */
function failure(code: string): Error {
  return Object.assign(new Error('the plugin failed'), { code })
}

describe('the scan of a QR code', () => {
  it('reads one QR code and answers its text', async () => {
    const scan = scanner(async () => ({ ScanResult: 'https://a.example/sign-in#abc' }))

    await expect(scanQrCode(scan)).resolves.toBe('https://a.example/sign-in#abc')
    expect(scan).toHaveBeenCalledWith(expect.objectContaining({ hint: 0 }))
  })

  it('answers nothing when the Person closes the scanner', async () => {
    await expect(scanQrCode(scanner(async () => Promise.reject(failure('OS-PLUG-BARC-0006'))))).resolves.toBeNull()
  })

  it('tells the Person to allow the camera, or to paste the link', async () => {
    await expect(scanQrCode(scanner(async () => Promise.reject(failure('OS-PLUG-BARC-0007'))))).rejects.toThrow(
      'Pagis cannot use the camera. Allow the camera for Pagis in the Settings app, or paste the link.',
    )
  })

  it('tells the Person to try again, or to paste the link, after another failure', async () => {
    await expect(scanQrCode(scanner(async () => Promise.reject(failure('OS-PLUG-BARC-0004'))))).rejects.toThrow(
      'Pagis could not scan the QR code. Try again, or paste the link.',
    )
  })
})
