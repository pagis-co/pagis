/**
 * The scan of the QR code of a Sign-In Link, with the native full-screen
 * scanner of `@capacitor/barcode-scanner`.
 */

import {
  CapacitorBarcodeScanner,
  type CapacitorBarcodeScannerOptions,
  type CapacitorBarcodeScannerScanResult,
} from '@capacitor/barcode-scanner'

/** One scan: `CapacitorBarcodeScanner.scanBarcode`. */
export type BarcodeScanner = (options: CapacitorBarcodeScannerOptions) => Promise<CapacitorBarcodeScannerScanResult>

/** The hint of a QR code, `Html5QrcodeSupportedFormats.QR_CODE`. The
 *  enum of the plugin comes from its web scanner, which the app does not
 *  bundle. */
const QR_CODE = 0

/** The error codes of the plugin, `OS-PLUG-BARC-<number>`. */
const CANCELLED = 'OS-PLUG-BARC-0006'
const NO_CAMERA = 'OS-PLUG-BARC-0007'

/**
 * Scan one QR code, and answer its text, or null when the Person closes
 * the scanner. A failure is an error in words for the Person.
 */
export async function scanQrCode(
  scan: BarcodeScanner = (options) => CapacitorBarcodeScanner.scanBarcode(options),
): Promise<string | null> {
  try {
    const result = await scan({ hint: QR_CODE, scanInstructions: 'Scan the QR code of a sign-in link.' })
    return result.ScanResult
  } catch (error) {
    const code = (error as { code?: unknown } | null)?.code
    if (code === CANCELLED) return null
    if (code === NO_CAMERA) {
      throw new Error('Pagis cannot use the camera. Allow the camera for Pagis in the Settings app, or paste the link.')
    }
    throw new Error('Pagis could not scan the QR code. Try again, or paste the link.')
  }
}
