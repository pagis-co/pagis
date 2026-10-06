// The web app manifest and the icons of the installable Product App.
// Vite copies `public/` to the root of the build, so the daemon serves
// each of these files at the path that the manifest names.

import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { decode } from 'fast-png'
import { describe, expect, it } from 'vitest'

const uiDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const publicDir = resolve(uiDir, 'public')

type ManifestIcon = { src: string; sizes: string; type: string; purpose: string }

function manifest(): Record<string, unknown> & { icons: ManifestIcon[] } {
  return JSON.parse(readFileSync(resolve(publicDir, 'manifest.webmanifest'), 'utf8'))
}

function icon(file: string) {
  return decode(readFileSync(resolve(publicDir, file.replace(/^\//, ''))))
}

/** The light value of a token of `tokens.css`, through its aliases. */
function lightToken(name: string): string {
  const css = readFileSync(resolve(uiDir, 'src/tokens.css'), 'utf8')
  const light = css.slice(css.indexOf(':root {'), css.indexOf('@media'))
  const read = (token: string): string => {
    const found = new RegExp(`${token}:\\s*([^;]+);`).exec(light)?.[1].trim()
    if (found === undefined) throw new Error(`tokens.css has no light ${token}`)
    const alias = /^var\((--[\w-]+)\)$/.exec(found)
    return alias === null ? found : read(alias[1])
  }
  return read(name)
}

function hex(color: string): number[] {
  const n = Number.parseInt(color.slice(1), 16)
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255]
}

/** Each pixel as [x, y, red, green, blue, alpha]. */
function pixels(image: ReturnType<typeof decode>): number[][] {
  expect(image.channels).toBe(4)
  expect(image.depth).toBe(8)
  const out: number[][] = []
  for (let y = 0; y < image.height; y += 1) {
    for (let x = 0; x < image.width; x += 1) {
      const at = (y * image.width + x) * 4
      out.push([x, y, ...image.data.slice(at, at + 4)])
    }
  }
  return out
}

describe('the web app manifest', () => {
  it('installs the Product App at the root, in a window of its own', () => {
    expect(manifest()).toMatchObject({
      id: '/',
      name: 'Pagis',
      short_name: 'Pagis',
      start_url: '/',
      scope: '/',
      display: 'standalone',
    })
  })

  it('paints the launch screen and the title bar in the light ground', () => {
    const ground = lightToken('--ground')
    expect(ground).toMatch(/^#[0-9a-f]{6}$/i)
    expect(manifest().background_color).toBe(ground)
    expect(manifest().theme_color).toBe(ground)
  })

  it('names a 192 px and a 512 px icon, and a 512 px maskable icon', () => {
    expect(manifest().icons).toEqual([
      { src: '/icon-192.png', sizes: '192x192', type: 'image/png', purpose: 'any' },
      { src: '/icon-512.png', sizes: '512x512', type: 'image/png', purpose: 'any' },
      { src: '/icon-maskable-512.png', sizes: '512x512', type: 'image/png', purpose: 'maskable' },
    ])
  })

  it('names icon files of the size it states', () => {
    for (const entry of manifest().icons) {
      const [width, height] = entry.sizes.split('x').map(Number)
      const image = icon(entry.src)
      expect([image.width, image.height], entry.src).toEqual([width, height])
    }
  })
})

describe('the web app icons', () => {
  it('keep the mark inside the safe zone of the maskable icon, on a full ground', () => {
    const image = icon('icon-maskable-512.png')
    const ground = hex(lightToken('--ground'))
    const center = image.width / 2
    // A launcher can cut the icon to any shape that holds the circle of
    // 80 % of its width, so every pixel outside that circle is ground.
    const safe = image.width * 0.4
    for (const [x, y, red, green, blue, alpha] of pixels(image)) {
      expect(alpha, `alpha at ${x},${y}`).toBe(255)
      if (Math.hypot(x + 0.5 - center, y + 0.5 - center) > safe) {
        expect([red, green, blue], `ground at ${x},${y}`).toEqual(ground)
      }
    }
  })

  it('give iOS a 180 px touch icon with no transparent pixel', () => {
    const image = icon('apple-touch-icon.png')
    expect([image.width, image.height]).toEqual([180, 180])
    // iOS draws a transparent pixel black.
    expect(pixels(image).filter((pixel) => pixel[5] !== 255)).toEqual([])
  })

  it('give the Android status bar a 96 px badge in one ink on a transparent ground', () => {
    const image = icon('badge-96.png')
    expect([image.width, image.height]).toEqual([96, 96])
    const all = pixels(image)
    expect(all[0][5], 'the corner is transparent').toBe(0)
    const inked = all.filter((pixel) => pixel[5] > 0)
    expect(inked.length).toBeGreaterThan(0)
    const inks = new Set(inked.map(([, , red, green, blue]) => `${red},${green},${blue}`))
    expect([...inks]).toHaveLength(1)
  })
})
