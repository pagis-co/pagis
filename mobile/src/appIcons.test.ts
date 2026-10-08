import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { decode } from 'fast-png'
import { describe, expect, it } from 'vitest'

const mobile = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const resources = 'android/app/src/main/res/'
const appIcon = 'ios/App/App/Assets.xcassets/AppIcon.appiconset/'
const colors = [[0x5b, 0x49, 0xc0], [0xe0, 0x70, 0x4a], [0xc4, 0x84, 0x1a]]
const densities = [['mdpi', 48], ['hdpi', 72], ['xhdpi', 96], ['xxhdpi', 144], ['xxxhdpi', 192]] as const

function read(path: string): string {
  return readFileSync(resolve(mobile, path), 'utf8')
}

function icon(path: string) {
  return decode(readFileSync(resolve(mobile, path)))
}

function pixel(image: ReturnType<typeof decode>, x: number, y: number): number[] {
  const at = (y * image.width + x) * image.channels
  return [...image.data.slice(at, at + image.channels)]
}

function hasMark(image: ReturnType<typeof decode>) {
  for (const color of colors) {
    let found = false
    for (let at = 0; at < image.data.length; at += image.channels) {
      if (color.every((channel, index) => image.data[at + index] === channel)) {
        found = true
        break
      }
    }
    expect(found, `desk color ${color}`).toBe(true)
  }
  // The fourth place of the mark stays open.
  expect(pixel(image, Math.floor(image.width * 0.65), Math.floor(image.height * 0.65)).slice(0, 3))
    .toEqual([255, 255, 255])
}

describe('the Mobile App icons', () => {
  it('gives iPhone, iPad and the App Store a 1024 px Pagis icon with no alpha channel', () => {
    const catalog = JSON.parse(read(`${appIcon}Contents.json`))
    expect(catalog.images).toEqual([
      { filename: 'AppIcon-512@2x.png', idiom: 'universal', platform: 'ios', size: '1024x1024' },
    ])
    const image = icon(`${appIcon}${catalog.images[0].filename}`)
    expect([image.width, image.height]).toEqual([1024, 1024])
    expect(image.depth).toBe(8)
    expect(image.channels).toBe(3)
    hasMark(image)
    expect(read('ios/App/App.xcodeproj/project.pbxproj')).toContain('ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon;')
  })

  it.each(densities)('gives Android %s square, round and adaptive Pagis icons', (density, size) => {
    for (const name of ['ic_launcher', 'ic_launcher_round']) {
      const image = icon(`${resources}mipmap-${density}/${name}.png`)
      expect([image.width, image.height]).toEqual([size, size])
      hasMark(image)
      expect(pixel(image, 0, 0)[3]).toBe(name === 'ic_launcher_round' ? 0 : 255)
    }
    const foreground = icon(`${resources}mipmap-${density}/ic_launcher_foreground.png`)
    const fullSize = size * 108 / 48
    expect([foreground.width, foreground.height]).toEqual([fullSize, fullSize])
    expect(foreground.channels).toBe(4)
    expect(foreground.depth).toBe(8)
    // Android guarantees the central 66 dp circle of the 108 dp layer.
    // Every visible pixel of the mark must stay inside that circle.
    let outside = 0
    for (let y = 0; y < fullSize; y += 1) {
      for (let x = 0; x < fullSize; x += 1) {
        if (Math.hypot(x + 0.5 - fullSize / 2, y + 0.5 - fullSize / 2) > fullSize * 33 / 108 &&
            pixel(foreground, x, y)[3] !== 0) outside += 1
      }
    }
    expect(outside).toBe(0)
    for (const color of colors) {
      expect(foreground.data.some((_, at) => at % 4 === 0 &&
        color.every((channel, index) => foreground.data[at + index] === channel))).toBe(true)
    }
  })

  it('uses the adaptive layers for both launcher shapes and a monochrome layer on Android 13', () => {
    const manifest = read('android/app/src/main/AndroidManifest.xml')
    expect(manifest).toContain('android:icon="@mipmap/ic_launcher"')
    expect(manifest).toContain('android:roundIcon="@mipmap/ic_launcher_round"')
    for (const name of ['ic_launcher', 'ic_launcher_round']) {
      for (const version of [26, 33]) {
        const xml = read(`${resources}mipmap-anydpi-v${version}/${name}.xml`)
        expect(xml).toContain('<background android:drawable="@color/ic_launcher_background"')
        expect(xml).toContain('<foreground android:drawable="@mipmap/ic_launcher_foreground"')
        if (version === 33) {
          expect(xml).toContain('<monochrome android:drawable="@drawable/ic_launcher_monochrome"')
        }
      }
    }
    expect(read(`${resources}values/ic_launcher_background.xml`)).toContain('#FFFFFF')
    const monochrome = new DOMParser().parseFromString(read(`${resources}drawable/ic_launcher_monochrome.xml`), 'text/xml')
    expect(monochrome.querySelector('parsererror')).toBeNull()
    expect(monochrome.documentElement.getAttribute('android:viewportWidth')).toBe('270')
    expect(monochrome.documentElement.getAttribute('android:viewportHeight')).toBe('270')
    const group = monochrome.querySelector('group')!
    expect([group.getAttribute('android:translateX'), group.getAttribute('android:translateY')]).toEqual(['55', '55'])
    const notification = new DOMParser().parseFromString(read(`${resources}drawable/ic_notification.xml`), 'text/xml')
    const paths = (xml: Document) => [...xml.querySelectorAll('path')].map((path) => path.getAttribute('android:pathData'))
    expect(paths(monochrome)).toEqual(paths(notification))
    expect([...monochrome.querySelectorAll('path')].every((path) => path.getAttribute('android:fillColor') === '#FFFFFFFF')).toBe(true)
  })
})
