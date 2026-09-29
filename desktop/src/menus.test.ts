// The tray item and the application menu (ADR-0025).

import * as fs from 'node:fs'
import * as zlib from 'node:zlib'

import { describe, expect, it, vi } from 'vitest'

import { applicationMenu, renderTray, trayIcon } from './menus'

// The menus are built from Electron's own template shape, so the test
// takes the template back out of `buildFromTemplate` and reads it.
vi.mock('electron', () => ({
  Menu: { buildFromTemplate: (template: unknown) => template },
  Tray: class {
    setToolTip(): void {}
    setContextMenu(): void {}
  },
  app: { name: 'Pagis' },
  shell: { openExternal: vi.fn() },
}))

type Item = { label?: string; role?: string; accelerator?: string; click?: () => void; submenu?: Item[] }

function actions(openAdministration: () => void) {
  return {
    open: vi.fn(),
    openAdministration,
    quit: vi.fn(),
    openAtLogin: vi.fn(),
    isOpenAtLogin: () => false,
    newVersion: () => null,
  }
}

/** The item of a menu, by label, at any depth. */
function item(menu: Item[], label: string): Item | undefined {
  for (const entry of menu) {
    if (entry.label === label) return entry
    const found = entry.submenu ? item(entry.submenu, label) : undefined
    if (found) return found
  }
  return undefined
}

describe('the menus', () => {
  it('opens the Administration Interface from the application menu', () => {
    const openAdministration = vi.fn()

    const menu = applicationMenu(actions(openAdministration)) as unknown as Item[]

    const administration = item(menu, 'Administration')
    expect(administration).toBeDefined()
    administration?.click?.()
    expect(openAdministration).toHaveBeenCalled()
  })

  it('opens it from the tray item too', () => {
    const openAdministration = vi.fn()
    let menu: Item[] = []
    const tray = {
      setContextMenu: (built: unknown) => { menu = built as Item[] },
    }

    renderTray(tray as never, actions(openAdministration))

    const administration = item(menu, 'Administration')
    expect(administration).toBeDefined()
    administration?.click?.()
    expect(openAdministration).toHaveBeenCalled()
  })

  it('holds the macOS application roles on macOS alone', () => {
    const mac = applicationMenu(actions(vi.fn()), 'darwin') as unknown as Item[]
    const linux = applicationMenu(actions(vi.fn()), 'linux') as unknown as Item[]

    expect(mac[0].label).toBe('Pagis')
    expect(mac[0].submenu?.some((entry) => entry.role === 'hideOthers')).toBe(true)
    expect(linux[0].label).toBe('File')
    expect(linux[0].submenu?.map((entry) => entry.role)).not.toContain('hideOthers')
    const quit = item(linux, 'Quit Pagis')
    expect(quit?.accelerator).toBe('CmdOrCtrl+Q')
  })

  it('gives macOS a template icon and Linux a drawn one', () => {
    expect(trayIcon('darwin')).toMatch(/trayTemplate\.png$/)
    expect(trayIcon('linux')).toMatch(/tray\.png$/)
  })

  /** Electron reads a plain PNG as 1x points and loads the `@2x` file
   *  beside it for a Retina screen. The macOS menu bar holds 16 point
   *  icons. A Linux status area scales the one image that Chromium sends
   *  it to its own size, which is 22 px on KDE Plasma. */
  it('draws the tray icons at the size of the menu bar and the status area', () => {
    const template = trayIcon('darwin')
    expect(pngSize(template)).toEqual({ width: 16, height: 16 })
    expect(pngSize(template.replace(/\.png$/, '@2x.png'))).toEqual({ width: 32, height: 32 })
    expect(pngSize(trayIcon('linux'))).toEqual({ width: 22, height: 22 })
  })

  /** macOS tints a template image to the menu bar, and reads only its
   *  alpha: every pixel is black. */
  it('draws the macOS icons as template images', () => {
    for (const file of [trayIcon('darwin'), trayIcon('darwin').replace(/\.png$/, '@2x.png')]) {
      const pixels = pngPixels(file)
      expect(pixels.some((pixel) => pixel[3] > 0)).toBe(true)
      expect(pixels.every(([red, green, blue]) => red === 0 && green === 0 && blue === 0)).toBe(true)
    }
  })
})

function pngSize(file: string): { width: number; height: number } {
  const bytes = fs.readFileSync(file)
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) }
}

/** The RGBA pixels of an 8-bit RGBA PNG. */
function pngPixels(file: string): number[][] {
  const bytes = fs.readFileSync(file)
  const { width, height } = pngSize(file)
  expect([bytes[24], bytes[25]]).toEqual([8, 6])
  const chunks: Buffer[] = []
  for (let at = 8; at < bytes.length;) {
    const length = bytes.readUInt32BE(at)
    if (bytes.toString('latin1', at + 4, at + 8) === 'IDAT') chunks.push(bytes.subarray(at + 8, at + 8 + length))
    at += length + 12
  }
  const data = zlib.inflateSync(Buffer.concat(chunks))
  const stride = width * 4
  let previous = Buffer.alloc(stride)
  const pixels: number[][] = []
  for (let y = 0; y < height; y += 1) {
    const filter = data[y * (stride + 1)]
    const row = Buffer.from(data.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1)))
    for (let x = 0; x < stride; x += 1) {
      const left = x >= 4 ? row[x - 4] : 0
      const up = previous[x]
      const corner = x >= 4 ? previous[x - 4] : 0
      const predictor = [0, left, up, (left + up) >> 1, paeth(left, up, corner)][filter]
      row[x] = (row[x] + predictor) & 0xff
    }
    for (let x = 0; x < width; x += 1) pixels.push([...row.subarray(x * 4, x * 4 + 4)])
    previous = row
  }
  return pixels
}

function paeth(left: number, up: number, corner: number): number {
  const estimate = left + up - corner
  const [toLeft, toUp, toCorner] = [left, up, corner].map((value) => Math.abs(estimate - value))
  if (toLeft <= toUp && toLeft <= toCorner) return left
  return toUp <= toCorner ? up : corner
}
