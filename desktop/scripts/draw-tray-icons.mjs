// Draw the tray icons of the Client App from the Pagis mark
// (assets/brand/README.md): `node scripts/draw-tray-icons.mjs`.
//
// - static/trayTemplate.png, 16 px, and static/trayTemplate@2x.png,
//   32 px: the macOS menu bar holds 16 point icons, and Electron loads
//   the `@2x` file for a Retina screen. They are black template images,
//   so macOS tints them to the menu bar.
// - static/tray.png, 22 px, in the colors of the mark: a Linux status
//   area scales the one image that Chromium sends it to its own size,
//   which is 22 px on KDE Plasma.
//
// The script draws the three desks with no library: it samples each pixel
// on an 8 by 8 grid and writes the PNG with node:zlib.

import * as fs from 'node:fs'
import * as zlib from 'node:zlib'
import { fileURLToPath } from 'node:url'

// The drawing sits on the 160 unit square of the mark, and the desks fill
// its 120 unit box from 20 to 140. Each icon shows a square view of it.
// On the macOS icons, 8 units are one pixel at 16 px and two at 32 px, so
// each desk edge and each gap falls on a pixel edge and stays sharp. The
// 16 px view has its one pixel of margin at the top left. A Linux status
// area scales its icon, so that view keeps an even margin of 6 units.
const SHARP_16 = { x: 12, y: 12, size: 128 }
const SHARP_32 = { x: 16, y: 16, size: 128 }
const EVEN = { x: 14, y: 14, size: 132 }
const SAMPLES = 8

const LIGHT = { top: [0x5b, 0x49, 0xc0], bowl: [0xe0, 0x70, 0x4a], bottom: [0xc4, 0x84, 0x1a] }
const INK = { top: [0, 0, 0], bowl: [0, 0, 0], bottom: [0, 0, 0] }

/** The desk that holds the point, or null for the ground. */
function desk(x, y) {
  if (roundedSquare(x, y, 20, 20)) return 'top'
  if (roundedSquare(x, y, 20, 84)) return 'bottom'
  if (bowl(x, y)) return 'bowl'
  return null
}

/** A 56 unit square desk with a 10 unit corner radius. */
function roundedSquare(x, y, left, top) {
  return roundedLeft(x, y, left, top, left + 56, top + 56) && roundedRight(x, y, left + 56, top, top + 56)
}

/** The bowl: the left corners of a square desk, and a 28 unit half
 *  circle on its right. */
function bowl(x, y) {
  if (x >= 112) return (x - 112) ** 2 + (y - 48) ** 2 <= 28 ** 2
  return roundedLeft(x, y, 84, 20, 112, 76)
}

function roundedLeft(x, y, left, top, right, bottom) {
  if (x < left || x > right || y < top || y > bottom) return false
  return corner(x, y, left + 10, top + 10, x < left + 10 && y < top + 10) &&
    corner(x, y, left + 10, bottom - 10, x < left + 10 && y > bottom - 10)
}

function roundedRight(x, y, right, top, bottom) {
  return corner(x, y, right - 10, top + 10, x > right - 10 && y < top + 10) &&
    corner(x, y, right - 10, bottom - 10, x > right - 10 && y > bottom - 10)
}

function corner(x, y, centerX, centerY, inCorner) {
  return !inCorner || (x - centerX) ** 2 + (y - centerY) ** 2 <= 10 ** 2
}

/** The RGBA pixels of the view of the mark at `size` px, in the desk
 *  colors given. */
function draw(size, view, colors) {
  const pixels = Buffer.alloc(size * size * 4)
  const unit = view.size / size
  for (let row = 0; row < size; row += 1) {
    for (let column = 0; column < size; column += 1) {
      const sum = [0, 0, 0]
      let covered = 0
      for (let sampleY = 0; sampleY < SAMPLES; sampleY += 1) {
        for (let sampleX = 0; sampleX < SAMPLES; sampleX += 1) {
          const x = view.x + (column + (sampleX + 0.5) / SAMPLES) * unit
          const y = view.y + (row + (sampleY + 0.5) / SAMPLES) * unit
          const found = desk(x, y)
          if (found === null) continue
          covered += 1
          for (let channel = 0; channel < 3; channel += 1) sum[channel] += colors[found][channel]
        }
      }
      const at = (row * size + column) * 4
      if (covered === 0) continue
      for (let channel = 0; channel < 3; channel += 1) pixels[at + channel] = Math.round(sum[channel] / covered)
      pixels[at + 3] = Math.round((covered / (SAMPLES * SAMPLES)) * 255)
    }
  }
  return pixels
}

/** An 8-bit RGBA PNG. */
function png(size, pixels) {
  const rows = Buffer.alloc(size * (size * 4 + 1))
  for (let row = 0; row < size; row += 1) {
    pixels.copy(rows, row * (size * 4 + 1) + 1, row * size * 4, (row + 1) * size * 4)
  }
  const header = Buffer.alloc(13)
  header.writeUInt32BE(size, 0)
  header.writeUInt32BE(size, 4)
  header.set([8, 6, 0, 0, 0], 8)
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', zlib.deflateSync(rows, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ])
}

function chunk(type, data) {
  const length = Buffer.alloc(4)
  length.writeUInt32BE(data.length)
  const body = Buffer.concat([Buffer.from(type, 'latin1'), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(zlib.crc32(body))
  return Buffer.concat([length, body, crc])
}

const icons = [
  ['trayTemplate.png', 16, SHARP_16, INK],
  ['trayTemplate@2x.png', 32, SHARP_32, INK],
  ['tray.png', 22, EVEN, LIGHT],
]
for (const [name, size, view, colors] of icons) {
  const file = fileURLToPath(new URL(`../static/${name}`, import.meta.url))
  fs.writeFileSync(file, png(size, draw(size, view, colors)))
  console.log(`wrote ${name}, ${size} px`)
}
