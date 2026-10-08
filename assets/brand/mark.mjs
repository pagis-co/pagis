// Draw the Pagis mark from its construction (README.md, "Construction")
// into PNG files, with no library. The tray icons of the Client App
// (desktop/scripts/draw-tray-icons.mjs) and the icons of the Product App
// web app (ui/scripts/draw-web-app-icons.mjs), and the Mobile App icons
// (mobile/scripts/draw-app-icons.mjs) use this module.
//
// The drawing sits on the 160 unit square of the mark, and the desks fill
// its 120 unit box from 20 to 140. An icon shows a square view of that
// plane: `{ x, y, size }` in units. The module samples each pixel on an
// 8 by 8 grid and writes the PNG with node:zlib.

import * as zlib from 'node:zlib'

const SAMPLES = 8

/** The desk colors of the mark on a light ground (README.md, "Color"). */
export const LIGHT = { top: [0x5b, 0x49, 0xc0], bowl: [0xe0, 0x70, 0x4a], bottom: [0xc4, 0x84, 0x1a] }

/** The desk colors of the mark in one ink. */
export function oneInk(color) {
  return { top: color, bowl: color, bottom: color }
}

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

/** The test for a point in the square from `start` to `end` units on
 *  both axes, with corners of `radius` units. */
export function roundedTile(start, end, radius) {
  return (x, y) => {
    if (x < start || x > end || y < start || y > end) return false
    const centerX = Math.min(Math.max(x, start + radius), end - radius)
    const centerY = Math.min(Math.max(y, start + radius), end - radius)
    return (x - centerX) ** 2 + (y - centerY) ** 2 <= radius ** 2
  }
}

/** The RGBA pixels of the view of the mark at `size` px, in the desk
 *  colors given. Without `ground`, a point outside the desks is
 *  transparent. With `ground`, `{ color, covers(x, y) }`, a point outside
 *  the desks that `covers` holds takes the ground color. */
export function draw(size, view, colors, ground = null) {
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
          const color = found !== null ? colors[found] : ground?.covers(x, y) ? ground.color : null
          if (color === null) continue
          covered += 1
          for (let channel = 0; channel < 3; channel += 1) sum[channel] += color[channel]
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
export function png(size, pixels) {
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
