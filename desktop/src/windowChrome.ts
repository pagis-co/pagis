import type { BrowserWindowConstructorOptions, WebContents } from 'electron'

/** The title bar of the product window. On macOS the page runs up to the
 *  top edge and the window buttons sit over it, so the window turns on the
 *  Window Controls Overlay: the page reads the buttons' area from the
 *  `titlebar-area-*` CSS environment variables and keeps its own content
 *  out of it. Another platform keeps its standard frame. */
export function productWindowChrome(platform: NodeJS.Platform): BrowserWindowConstructorOptions {
  return platform === 'darwin' ? { titleBarStyle: 'hiddenInset', titleBarOverlay: true } : {}
}

/** The window of one of the Client App's own pages. It has the chrome of
 *  the product window, and its page runs sandboxed with only its preload. */
function pageWindowOptions(
  platform: NodeJS.Platform,
  page: { width: number; height: number; title: string; preload: string },
): BrowserWindowConstructorOptions {
  return {
    width: page.width,
    height: page.height,
    title: page.title,
    ...productWindowChrome(platform),
    webPreferences: {
      preload: page.preload,
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
    },
  }
}

/** The setup window. It has a fixed size, as the first-run assistant of
 *  macOS, Docker Desktop and Tailscale has. Its height is that of the
 *  tallest regular screen: the first screen with "Connect to a Pagis
 *  server" and the longest problem under the Server address field
 *  measures 580 px with the title bar strip and the footer. So each
 *  regular screen fits, the page centres the shorter ones, and only a
 *  long failure scrolls, in the middle area. The size is of the page,
 *  without the frame that Linux draws. */
export function setupWindowOptions(platform: NodeJS.Platform, preload: string): BrowserWindowConstructorOptions {
  return {
    ...pageWindowOptions(platform, { width: 640, height: 590, title: 'Set up Pagis', preload }),
    useContentSize: true,
    resizable: false,
    maximizable: false,
    fullscreenable: false,
  }
}

export function statusWindowOptions(platform: NodeJS.Platform, preload: string): BrowserWindowConstructorOptions {
  return pageWindowOptions(platform, { width: 620, height: 460, title: 'Pagis', preload })
}

/** How the product window gathers its WebRTC candidates for the live
 *  screen (ADR-0014). Electron's default binds one socket to each network
 *  interface, and on macOS a socket bound to the LAN or to the Colima
 *  bridge cannot reach the `127.0.0.1` candidate that the Media Relay of
 *  a local installation offers, so the screen never connects. This
 *  policy is what Chromium applies to a page: WebRTC enumerates no
 *  interface and binds to the any address on the default route, which
 *  reaches loopback and a remote relay alike. */
export const PRODUCT_WEBRTC_IP_POLICY = 'default_public_and_private_interfaces'

export function applyProductWebRtcPolicy(
  contents: Pick<WebContents, 'setWebRTCIPHandlingPolicy'>,
): void {
  contents.setWebRTCIPHandlingPolicy(PRODUCT_WEBRTC_IP_POLICY)
}
