/**
 * The web permissions of the Client App.
 *
 * Electron grants each permission request and each permission check that
 * no handler decides, and it shows no prompt. A browser asks the Person
 * first. So the Client App decides each one and denies by default. A
 * permission that no handler grants cannot go to script that an attacker
 * runs at the Product App origin.
 *
 * The Product App uses two permissions: the microphone for dictation
 * (ADR-0025), and clipboard write for the Copy action. The Client App
 * grants them only to the main frame of the product window at its
 * Product App origin. The administration, setup and status windows use
 * the same session and get no permission. This is the Electron security
 * checklist item "Handle session permission requests from remote
 * content", which Signal Desktop and VS Code also follow.
 *
 * Web Bluetooth does not go through these handlers. Electron gives the
 * first device that a scan finds to the page when a `select-bluetooth-device`
 * listener does not prevent the default, and the Person sees no chooser.
 * Electron cancels the request only while its own listener is the one
 * listener on the event. The Product App uses no Bluetooth, so each window
 * of the Client App cancels each device request, and the session refuses
 * each Bluetooth pairing.
 */

import { isTrustedServerOrigin } from './origin'
import { sameProductOrigin } from './recoveryView'

/** The part of a WebContents that the decision reads. */
interface AskingContents {
  mainFrame: { origin: string }
}

/** The product window, and the Product App origin that the Client App
 *  loaded into it. */
export interface ProductWindow {
  contents: AskingContents
  origin: string
}

/** What Electron gives with a permission request. */
interface RequestDetails {
  isMainFrame: boolean
  requestingUrl: string
  /** For `media`: the devices that the page asks for. */
  mediaTypes?: readonly string[]
  securityOrigin?: string
}

/** What Electron gives with a permission check. */
interface CheckDetails {
  isMainFrame: boolean
  requestingUrl?: string
  /** For `media`: the device that the page checks. */
  mediaType?: string
  securityOrigin?: string
}

type RequestHandler = (
  contents: AskingContents,
  permission: string,
  callback: (granted: boolean) => void,
  details: RequestDetails,
) => void

type CheckHandler = (
  contents: AskingContents | null,
  permission: string,
  requestingOrigin: string,
  details: CheckDetails,
) => boolean

/** What Electron gives with a Bluetooth pairing that needs a confirmation
 *  or a PIN. */
interface PairingDetails {
  deviceId: string
  pairingKind: 'confirm' | 'confirmPin' | 'providePin'
  pin?: string
}

type PairingHandler = (
  details: PairingDetails,
  callback: (response: { confirmed: boolean }) => void,
) => void

/** The part of an Electron session that holds the permission handlers. */
export interface PermissionSession {
  setPermissionRequestHandler(handler: RequestHandler): void
  setPermissionCheckHandler(handler: CheckHandler): void
  setDevicePermissionHandler(handler: () => boolean): void
  setBluetoothPairingHandler(handler: PairingHandler): void
}

/** A Bluetooth device that a scan found. */
interface BluetoothDevice {
  deviceId: string
  deviceName: string
}

/** What Electron gives with `select-bluetooth-device`. */
type ChooserListener = (
  event: { preventDefault(): void },
  devices: BluetoothDevice[],
  callback: (deviceId: string) => void,
) => void

/** The part of a WebContents that asks for a Bluetooth device. */
export interface BluetoothContents {
  on(event: 'select-bluetooth-device', listener: ChooserListener): unknown
}

/** The platforms where Electron asks the pairing handler about a
 *  Bluetooth pairing that needs a confirmation or a PIN: Linux and
 *  Windows. macOS does the pairing itself. The session has the method on
 *  macOS too, but Electron documents it for Linux and Windows only. */
const PAIRING_HANDLER_PLATFORMS: readonly NodeJS.Platform[] = ['linux', 'win32']

/** Whether the Client App grants a permission request. */
export function grantsRequest(
  product: ProductWindow | null,
  contents: AskingContents,
  permission: string,
  details: RequestDetails,
): boolean {
  return fromProductApp(product, contents, details.isMainFrame, [details.requestingUrl, details.securityOrigin])
    && productAppUses(permission, details.mediaTypes ?? [])
}

/** Whether the Client App grants a permission check. */
export function grantsCheck(
  product: ProductWindow | null,
  contents: AskingContents | null,
  permission: string,
  requestingOrigin: string,
  details: CheckDetails,
): boolean {
  const media = details.mediaType === undefined ? [] : [details.mediaType]
  return fromProductApp(
    product, contents, details.isMainFrame,
    [requestingOrigin, details.requestingUrl, details.securityOrigin],
  ) && productAppUses(permission, media)
}

/**
 * Set the permission handlers on the session. Call it one time for each
 * session, before the first window loads. `product` gives the product
 * window at the time of each ask, because the window opens later and a
 * Local Installation that recovers can move it to another port.
 * `platform` is the platform that the Client App runs on.
 */
export function installPermissionHandlers(
  session: PermissionSession,
  product: () => ProductWindow | null,
  platform: NodeJS.Platform,
): void {
  session.setPermissionRequestHandler((contents, permission, callback, details) => {
    callback(grantsRequest(product(), contents, permission, details))
  })
  session.setPermissionCheckHandler((contents, permission, requestingOrigin, details) =>
    grantsCheck(product(), contents, permission, requestingOrigin, details))
  // A page gets no HID, serial or USB device, also one that a device
  // chooser gave it before.
  session.setDevicePermissionHandler(() => false)
  // Electron cancels a pairing when no handler is set. The Client App
  // refuses each pairing with its own handler, so that the refusal does
  // not depend on the default of an Electron release.
  if (PAIRING_HANDLER_PLATFORMS.includes(platform)) {
    session.setBluetoothPairingHandler((_details, callback) => { callback({ confirmed: false }) })
  }
}

/**
 * Cancel each Bluetooth device request of one window. Call it one time
 * for each window. The listener prevents the default, so Electron gives
 * no device to the page, and it answers with no device id, which cancels
 * the request. The page gets a `NotFoundError` and the Person sees no
 * chooser.
 */
export function installBluetoothRefusal(contents: BluetoothContents): void {
  contents.on('select-bluetooth-device', (event, _devices, callback) => {
    event.preventDefault()
    callback('')
  })
}

/**
 * Whether the main frame of the product window asks, at the Product App
 * origin that the origin rule trusts. `named` holds each origin or URL
 * that Electron gives for the asking frame, and each one must be that
 * origin. The frame's own origin must be that origin too. A page that a
 * `Content-Security-Policy: sandbox` answer gives an opaque origin keeps
 * its URL at the Product App origin, and Electron puts that URL in
 * `requestingUrl`.
 */
function fromProductApp(
  product: ProductWindow | null,
  contents: AskingContents | null,
  isMainFrame: boolean,
  named: readonly (string | undefined)[],
): boolean {
  if (product === null || contents !== product.contents || !isMainFrame) return false
  if (!isTrustedServerOrigin(product.origin)) return false
  return [product.contents.mainFrame.origin, ...named]
    .every((origin) => origin === undefined || sameProductOrigin(product.origin, origin))
}

/**
 * Whether the Product App uses the permission: clipboard write, or
 * `media` for the microphone alone. Electron names only the microphone
 * (`audio`) and the camera (`video`) in the media of a request. A request
 * that names no device, such as a screen capture, gets a denial.
 */
function productAppUses(permission: string, media: readonly string[]): boolean {
  if (permission === 'clipboard-sanitized-write') return true
  return permission === 'media' && media.length > 0 && media.every((device) => device === 'audio')
}
