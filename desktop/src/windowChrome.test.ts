import { describe, expect, it, vi } from 'vitest'

import {
  applyProductWebRtcPolicy,
  productWindowChrome,
  setupWindowOptions,
  statusWindowOptions,
} from './windowChrome'

describe('the product window chrome', () => {
  it('gives the page the window buttons area on macOS', () => {
    expect(productWindowChrome('darwin')).toEqual({
      titleBarStyle: 'hiddenInset',
      titleBarOverlay: true,
    })
  })

  it('keeps the standard frame elsewhere', () => {
    expect(productWindowChrome('linux')).toEqual({})
  })
})

// The setup and status pages are the first windows of the Client App. They
// have the frame of the product window, so the first run does not change
// its title bar when the product opens.
describe('the setup and status windows', () => {
  const windows = [
    ['setup', setupWindowOptions],
    ['status', statusWindowOptions],
  ] as const

  it.each(windows)('the %s window has the product window chrome on macOS', (_name, options) => {
    expect(options('darwin', '/app/preload.js')).toMatchObject({
      titleBarStyle: 'hiddenInset',
      titleBarOverlay: true,
    })
  })

  it.each(windows)('the %s window keeps the standard frame elsewhere', (_name, options) => {
    const chrome = options('linux', '/app/preload.js')

    expect(chrome).not.toHaveProperty('titleBarStyle')
    expect(chrome).not.toHaveProperty('titleBarOverlay')
  })

  it.each(windows)('the %s window runs its page sandboxed with its preload only', (_name, options) => {
    expect(options('darwin', '/app/preload.js').webPreferences).toEqual({
      preload: '/app/preload.js',
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
    })
  })

  /** The setup is a short flow of screens in a window of fixed size, as
   *  a first-run assistant is. The window is as high as its tallest
   *  regular screen: the first screen with "Connect to a Pagis server",
   *  the trust line and the longest problem under the Server address
   *  field. In Electron that screen measures 580 px with the title bar
   *  strip and the footer, in the light and the dark theme. So no regular
   *  screen scrolls, and the page centres the shorter ones. */
  it('gives the setup window a fixed size that fits its tallest regular screen', () => {
    for (const platform of ['darwin', 'linux'] as const) {
      expect(setupWindowOptions(platform, '/app/preload.js'), platform).toMatchObject({
        width: 640,
        height: 590,
        useContentSize: true,
        resizable: false,
        maximizable: false,
        fullscreenable: false,
      })
    }
  })

  it('names each window', () => {
    expect(setupWindowOptions('darwin', '/app/preload.js').title).toBe('Set up Pagis')
    expect(statusWindowOptions('darwin', '/app/preload.js').title).toBe('Pagis')
  })
})

describe('the product window WebRTC policy', () => {
  // Electron's default binds a socket to each interface, and on macOS
  // none of them reaches the loopback candidate of a local installation.
  it('binds to the default route, as Chromium does for a page', () => {
    const contents = { setWebRTCIPHandlingPolicy: vi.fn() }

    applyProductWebRtcPolicy(contents)

    expect(contents.setWebRTCIPHandlingPolicy).toHaveBeenCalledWith(
      'default_public_and_private_interfaces',
    )
  })
})
