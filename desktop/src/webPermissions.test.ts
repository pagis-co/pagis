// The web permissions of the Client App. Electron grants each permission
// that no handler decides, and it shows no prompt. So the Client App
// decides each one. The main frame of the product window at the Product
// App origin gets the microphone for dictation and clipboard write for the
// Copy action. Every other ask gets a denial. Each window cancels each
// Bluetooth device request, and the session refuses each Bluetooth pairing.

import { describe, expect, it } from 'vitest'

import {
  grantsCheck,
  grantsRequest,
  installBluetoothRefusal,
  installPermissionHandlers,
  type BluetoothContents,
  type PermissionSession,
  type ProductWindow,
} from './webPermissions'

/** The Product App origin of a Local Installation, as the Client App keeps it. */
const LOCAL = 'http://127.0.0.1:4400'
const SERVER = 'https://pagis.example.com'

const productContents = { mainFrame: { origin: LOCAL } }
const product: ProductWindow = { contents: productContents, origin: LOCAL }

/** The Administration Interface answers on the Administration Port of the same host. */
const administrationContents = { mainFrame: { origin: 'http://127.0.0.1:4401' } }

/** What Electron gives for `getUserMedia({ audio: true })` in the product window. */
const microphone = {
  isMainFrame: true,
  requestingUrl: `${LOCAL}/channels/01ABC`,
  mediaTypes: ['audio'],
  securityOrigin: `${LOCAL}/`,
}

/** What Electron gives when the main frame checks the microphone. */
const microphoneCheck = {
  isMainFrame: true,
  requestingUrl: `${LOCAL}/channels/01ABC`,
  mediaType: 'audio',
  securityOrigin: `${LOCAL}/`,
  embeddingOrigin: `${LOCAL}/`,
}

/** What Electron gives for `navigator.clipboard.writeText` in the main frame. */
const clipboardWrite = { isMainFrame: true, requestingUrl: `${LOCAL}/channels/01ABC` }

describe('a permission that the Product App uses', () => {
  it('grants a microphone request from the main frame of the product window at the Product App origin', () => {
    expect(grantsRequest(product, productContents, 'media', microphone)).toBe(true)
  })

  it('grants a microphone check from the same window and origin', () => {
    expect(grantsCheck(product, productContents, 'media', `${LOCAL}/`, microphoneCheck)).toBe(true)
  })

  it('grants clipboard write to the same window and origin', () => {
    expect(grantsRequest(product, productContents, 'clipboard-sanitized-write', clipboardWrite)).toBe(true)
  })

  /** A Client App connected to a Server shows the Product App at the
   *  Server's https:// origin. */
  it('grants the same permissions at the origin of a connected Server', () => {
    const contents = { mainFrame: { origin: SERVER } }
    const server: ProductWindow = { contents, origin: SERVER }
    const at = { requestingUrl: `${SERVER}/channels/01ABC`, securityOrigin: `${SERVER}/` }

    expect(grantsRequest(server, contents, 'media', { ...microphone, ...at })).toBe(true)
    expect(grantsCheck(server, contents, 'media', `${SERVER}/`, { ...microphoneCheck, ...at })).toBe(true)
    expect(grantsRequest(server, contents, 'clipboard-sanitized-write', { ...clipboardWrite, ...at })).toBe(true)
  })
})

describe('a permission that the Client App denies', () => {
  it('denies a media request that includes the camera', () => {
    expect(grantsRequest(product, productContents, 'media', { ...microphone, mediaTypes: ['audio', 'video'] })).toBe(false)
    expect(grantsRequest(product, productContents, 'media', { ...microphone, mediaTypes: ['video'] })).toBe(false)
  })

  it('denies a media check for the camera or for an unknown device', () => {
    expect(grantsCheck(product, productContents, 'media', `${LOCAL}/`, { ...microphoneCheck, mediaType: 'video' })).toBe(false)
    expect(grantsCheck(product, productContents, 'media', `${LOCAL}/`, { ...microphoneCheck, mediaType: 'unknown' })).toBe(false)
    expect(grantsCheck(product, productContents, 'media', `${LOCAL}/`, { ...microphoneCheck, mediaType: undefined })).toBe(false)
  })

  /** Electron names only the microphone and the camera in `mediaTypes`.
   *  A screen or a window capture names neither of them. */
  it('denies a media request that names no microphone', () => {
    expect(grantsRequest(product, productContents, 'media', { ...microphone, mediaTypes: [] })).toBe(false)
    expect(grantsRequest(product, productContents, 'media', { ...microphone, mediaTypes: undefined })).toBe(false)
  })

  /** The permission rule does not depend on the navigation rule. A
   *  product window whose main frame shows another origin gets a denial. */
  it('denies a request from another origin', () => {
    const attacker = 'https://attacker.example'
    const elsewhere = { requestingUrl: `${attacker}/`, securityOrigin: `${attacker}/` }
    const redirected = { mainFrame: { origin: attacker } }
    const window: ProductWindow = { contents: redirected, origin: LOCAL }

    expect(grantsRequest(window, redirected, 'media', { ...microphone, ...elsewhere })).toBe(false)
    expect(grantsRequest(window, redirected, 'clipboard-sanitized-write', { ...clipboardWrite, ...elsewhere })).toBe(false)
    expect(grantsCheck(window, redirected, 'media', `${attacker}/`, { ...microphoneCheck, ...elsewhere })).toBe(false)
  })

  /** Electron gives an origin or a URL in more than one place, and each
   *  one must be the Product App origin. */
  it('denies an ask that names another origin in any place', () => {
    const attacker = 'https://attacker.example/'

    expect(grantsRequest(product, productContents, 'media', { ...microphone, requestingUrl: attacker })).toBe(false)
    expect(grantsRequest(product, productContents, 'media', { ...microphone, securityOrigin: attacker })).toBe(false)
    expect(grantsCheck(product, productContents, 'media', attacker, microphoneCheck)).toBe(false)
    expect(grantsCheck(product, productContents, 'media', `${LOCAL}/`, { ...microphoneCheck, requestingUrl: attacker })).toBe(false)
    expect(grantsCheck(product, productContents, 'media', `${LOCAL}/`, { ...microphoneCheck, securityOrigin: attacker })).toBe(false)
  })

  /** A page that a `Content-Security-Policy: sandbox` answer gives an
   *  opaque origin keeps its URL at the Product App origin, and Electron
   *  puts that URL in `requestingUrl`. So the frame's own origin decides
   *  too. */
  it('denies the product window while its main frame has an opaque origin', () => {
    const sandboxed = { mainFrame: { origin: 'null' } }
    const window: ProductWindow = { contents: sandboxed, origin: LOCAL }
    const artifact = { requestingUrl: `${LOCAL}/api/v1/artifacts/01ABC` }

    expect(grantsRequest(window, sandboxed, 'clipboard-sanitized-write', { ...clipboardWrite, ...artifact })).toBe(false)
    expect(grantsRequest(window, sandboxed, 'media', { ...microphone, ...artifact })).toBe(false)
    expect(grantsCheck(window, sandboxed, 'media', `${LOCAL}/`, { ...microphoneCheck, ...artifact })).toBe(false)
  })

  it('denies a sub-frame at the Product App origin', () => {
    expect(grantsRequest(product, productContents, 'media', { ...microphone, isMainFrame: false })).toBe(false)
    expect(grantsCheck(product, productContents, 'media', `${LOCAL}/`, { ...microphoneCheck, isMainFrame: false })).toBe(false)
    expect(grantsRequest(product, productContents, 'clipboard-sanitized-write', { ...clipboardWrite, isMainFrame: false })).toBe(false)
  })

  /** The administration window uses the same session. Another window can
   *  also be at the Product App origin, so the window decides as well. */
  it('denies the administration window and every other window', () => {
    const administration = { requestingUrl: 'http://127.0.0.1:4401/settings', securityOrigin: 'http://127.0.0.1:4401/' }
    expect(grantsRequest(product, administrationContents, 'media', { ...microphone, ...administration })).toBe(false)
    expect(grantsRequest(product, administrationContents, 'clipboard-sanitized-write', { ...clipboardWrite, ...administration })).toBe(false)

    const another = { mainFrame: { origin: LOCAL } }
    expect(grantsRequest(product, another, 'media', microphone)).toBe(false)
    expect(grantsCheck(product, another, 'media', `${LOCAL}/`, microphoneCheck)).toBe(false)
    expect(grantsRequest(product, another, 'clipboard-sanitized-write', clipboardWrite)).toBe(false)

    // A check that no window makes, such as one from a service worker.
    expect(grantsCheck(product, null, 'media', `${LOCAL}/`, microphoneCheck)).toBe(false)
  })

  it('denies every other permission, also one that Electron does not name', () => {
    const others = [
      'notifications', 'geolocation', 'clipboard-read', 'openExternal', 'display-capture',
      'fullscreen', 'hid', 'serial', 'usb', 'unknown', 'a-permission-that-electron-does-not-name',
    ]
    for (const permission of others) {
      expect(grantsRequest(product, productContents, permission, clipboardWrite), permission).toBe(false)
      expect(grantsCheck(product, productContents, permission, `${LOCAL}/`, microphoneCheck), permission).toBe(false)
    }
  })

  it('denies each ask while no product window is open', () => {
    expect(grantsRequest(null, productContents, 'media', microphone)).toBe(false)
    expect(grantsCheck(null, productContents, 'media', `${LOCAL}/`, microphoneCheck)).toBe(false)
    expect(grantsRequest(null, productContents, 'clipboard-sanitized-write', clipboardWrite)).toBe(false)
  })

  /** One rule, `isTrustedServerOrigin`, decides which origin the Client
   *  App trusts, and the handlers apply it to the product window too. */
  it('denies a product window at an origin that the origin rule does not trust', () => {
    const clearText = 'http://192.168.1.10:4400'
    const contents = { mainFrame: { origin: clearText } }
    const untrusted: ProductWindow = { contents, origin: clearText }
    const at = { requestingUrl: `${clearText}/channels/01ABC`, securityOrigin: `${clearText}/` }

    expect(grantsRequest(untrusted, contents, 'media', { ...microphone, ...at })).toBe(false)
    expect(grantsCheck(untrusted, contents, 'media', `${clearText}/`, { ...microphoneCheck, ...at })).toBe(false)
    expect(grantsRequest(untrusted, contents, 'clipboard-sanitized-write', { ...clipboardWrite, ...at })).toBe(false)
  })

  /** Electron checks some permissions while a page loads, before the
   *  page has an origin. */
  it('denies a check that names no origin', () => {
    expect(grantsCheck(product, productContents, 'media', '', { ...microphoneCheck, requestingUrl: '', securityOrigin: undefined })).toBe(false)
  })
})

type RequestHandler = Parameters<PermissionSession['setPermissionRequestHandler']>[0]
type CheckHandler = Parameters<PermissionSession['setPermissionCheckHandler']>[0]
type PairingHandler = Parameters<PermissionSession['setBluetoothPairingHandler']>[0]

/** A session that keeps the handlers the Client App sets on it. */
function fakeSession() {
  const handlers: {
    request?: RequestHandler, check?: CheckHandler, device?: () => boolean, pairing?: PairingHandler,
  } = {}
  const session: PermissionSession = {
    setPermissionRequestHandler: (handler) => { handlers.request = handler },
    setPermissionCheckHandler: (handler) => { handlers.check = handler },
    setDevicePermissionHandler: (handler) => { handlers.device = handler },
    setBluetoothPairingHandler: (handler) => { handlers.pairing = handler },
  }
  return { session, handlers }
}

/** The answer that a request handler gives through its callback. */
function answer(
  handler: RequestHandler | undefined,
  contents: Parameters<RequestHandler>[0],
  permission: string,
  details: Parameters<RequestHandler>[3],
): boolean | undefined {
  let granted: boolean | undefined
  handler?.(contents, permission, (value) => { granted = value }, details)
  return granted
}

describe('the permission handlers of the Client App', () => {
  it('sets the request, check and device handlers on the session it gets', () => {
    const { session, handlers } = fakeSession()

    installPermissionHandlers(session, () => product, 'darwin')

    expect(handlers.request).toBeTypeOf('function')
    expect(handlers.check).toBeTypeOf('function')
    expect(handlers.device).toBeTypeOf('function')

    expect(answer(handlers.request, productContents, 'media', microphone)).toBe(true)
    expect(answer(handlers.request, productContents, 'notifications', clipboardWrite)).toBe(false)
    expect(answer(handlers.request, administrationContents, 'clipboard-sanitized-write', clipboardWrite)).toBe(false)
    expect(handlers.check?.(productContents, 'media', `${LOCAL}/`, microphoneCheck)).toBe(true)
    expect(handlers.check?.(productContents, 'media', `${LOCAL}/`, { ...microphoneCheck, mediaType: 'video' })).toBe(false)
    // A HID, serial or USB device reaches no page.
    expect(handlers.device?.()).toBe(false)
  })

  /** The product window opens after the handlers are set, and it can
   *  move to another port when a Local Installation recovers. */
  it('reads the product window again at each ask', () => {
    const { session, handlers } = fakeSession()
    let open: ProductWindow | null = null

    installPermissionHandlers(session, () => open, 'darwin')
    expect(answer(handlers.request, productContents, 'media', microphone)).toBe(false)

    open = product
    expect(answer(handlers.request, productContents, 'media', microphone)).toBe(true)

    const moved = 'http://127.0.0.1:4402'
    open = { contents: productContents, origin: moved }
    expect(answer(handlers.request, productContents, 'media', microphone)).toBe(false)
    expect(handlers.check?.(productContents, 'media', `${LOCAL}/`, microphoneCheck)).toBe(false)
  })
})

type ChooserListener = Parameters<BluetoothContents['on']>[1]

/** A WebContents that keeps the Bluetooth listeners that the Client App
 *  adds to it. */
function fakeWindow() {
  const listeners: ChooserListener[] = []
  const contents: BluetoothContents = {
    on: (_event, listener) => { listeners.push(listener) },
  }
  return { contents, listeners }
}

/** Emit `select-bluetooth-device` as Electron does. The result tells
 *  whether a listener prevented the default, and each device id that a
 *  listener answered. */
function choose(listeners: ChooserListener[], devices: { deviceId: string, deviceName: string }[]) {
  let prevented = false
  const answers: string[] = []
  for (const listener of listeners) {
    listener({ preventDefault: () => { prevented = true } }, devices, (deviceId) => answers.push(deviceId))
  }
  return { prevented, answers }
}

/** The answer that a pairing handler gives through its callback. */
function pair(handler: PairingHandler | undefined, pairingKind: 'confirm' | 'confirmPin' | 'providePin') {
  let response: unknown
  handler?.({ deviceId: '8A:3F:21:00:5C:9E', pairingKind, pin: '123456' }, (value) => { response = value })
  return response
}

describe('the Bluetooth handlers of the Client App', () => {
  const lock = { deviceId: '8A:3F:21:00:5C:9E', deviceName: 'Smart lock' }
  const tracker = { deviceId: '4D:10:7B:E2:91:03', deviceName: 'Fitness tracker' }

  /** Electron gives the first device that it finds to the page when a
   *  listener does not prevent the default. The Product App uses no
   *  Bluetooth, so each window cancels each request. Electron emits the
   *  event with no device when a scan starts, and again for each device
   *  that the scan finds. */
  it('registers a select-bluetooth-device listener that prevents the default and answers with an empty device id', () => {
    const { contents, listeners } = fakeWindow()

    installBluetoothRefusal(contents)

    expect(listeners).toHaveLength(1)
    expect(choose(listeners, [])).toEqual({ prevented: true, answers: [''] })
    expect(choose(listeners, [lock])).toEqual({ prevented: true, answers: [''] })
    expect(choose(listeners, [lock, tracker])).toEqual({ prevented: true, answers: [''] })
  })

  /** Electron calls the pairing handler on Linux and Windows for a device
   *  that needs a confirmation or a PIN. */
  it('sets a Bluetooth pairing handler that refuses each pairing on Linux and Windows', () => {
    for (const platform of ['linux', 'win32'] as const) {
      const { session, handlers } = fakeSession()

      installPermissionHandlers(session, () => product, platform)

      expect(handlers.pairing, platform).toBeTypeOf('function')
      expect(pair(handlers.pairing, 'confirm'), platform).toEqual({ confirmed: false })
      expect(pair(handlers.pairing, 'confirmPin'), platform).toEqual({ confirmed: false })
      expect(pair(handlers.pairing, 'providePin'), platform).toEqual({ confirmed: false })
    }
  })

  /** macOS does the pairing itself. Electron documents the pairing
   *  handler for Linux and Windows only. */
  it('sets no Bluetooth pairing handler on macOS', () => {
    const { session, handlers } = fakeSession()

    installPermissionHandlers(session, () => product, 'darwin')

    expect(handlers.pairing).toBeUndefined()
  })
})
