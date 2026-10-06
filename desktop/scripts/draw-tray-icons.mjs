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
// assets/brand/mark.mjs draws the mark.

import * as fs from 'node:fs'
import { fileURLToPath } from 'node:url'

import { LIGHT, draw, oneInk, png } from '../../assets/brand/mark.mjs'

// Each icon shows a square view of the 160 unit square of the mark. On
// the macOS icons, 8 units are one pixel at 16 px and two at 32 px, so
// each desk edge and each gap falls on a pixel edge and stays sharp. The
// 16 px view has its one pixel of margin at the top left. A Linux status
// area scales its icon, so that view keeps an even margin of 6 units.
const SHARP_16 = { x: 12, y: 12, size: 128 }
const SHARP_32 = { x: 16, y: 16, size: 128 }
const EVEN = { x: 14, y: 14, size: 132 }

const INK = oneInk([0, 0, 0])

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
