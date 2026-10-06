// Draw the icons of the Product App web app from the Pagis mark
// (assets/brand/README.md): `node scripts/draw-web-app-icons.mjs`.
// `public/manifest.webmanifest` and `index.html` name these files.
//
// - public/icon-192.png and public/icon-512.png: the Client App icon
//   (assets/brand/pagis-app-icon.svg), the mark on a white rounded tile
//   with a transparent margin. A browser shows it as it is.
// - public/icon-maskable-512.png: the mark inside the safe zone, the
//   circle of 80 % of the width, on a full ground. A launcher cuts the
//   icon to its own shape.
// - public/apple-touch-icon.png, 180 px: the same layout. iOS rounds the
//   corners itself and draws a transparent pixel black, so the icon has
//   no transparent pixel.
// - public/badge-96.png: the mark in one ink on a transparent ground.
//   The Android status bar shows only its shape.
//
// assets/brand/mark.mjs draws the mark.

import * as fs from 'node:fs'
import { fileURLToPath } from 'node:url'

import { LIGHT, draw, oneInk, png, roundedTile } from '../../assets/brand/mark.mjs'

// The light `--ground` of src/tokens.css.
const GROUND = [0xf7, 0xf6, 0xf2]
const WHITE = [0xff, 0xff, 0xff]
// The one ink of assets/brand/pagis-mark-mono.svg.
const MONO = oneInk([0x27, 0x25, 0x21])

// pagis-app-icon.svg is 1024 px square. It scales the mark by 4.12 and
// moves it 182.4 px, and its tile is 824 px square at 100 px with a
// 185 px corner radius. In units of the mark, the tile is the square
// from -20 to 180.
const SCALE = 4.12
const APP_ICON = { x: -182.4 / SCALE, y: -182.4 / SCALE, size: 1024 / SCALE }
const TILE = { color: WHITE, covers: roundedTile(-20, 180, 185 / SCALE) }

// The point of the mark farthest from its center (80, 80) is on the
// outer corner of a square desk, 80.7 units away. The safe zone has a
// radius of 40 % of the view, so a view of 208 units holds the mark with
// a margin.
const SAFE = { x: -24, y: -24, size: 208 }
const FULL_GROUND = { color: GROUND, covers: () => true }

// A status bar icon keeps 2 of its 24 points clear on each side.
const BADGE = { x: 8, y: 8, size: 144 }

const icons = [
  ['icon-192.png', 192, APP_ICON, LIGHT, TILE],
  ['icon-512.png', 512, APP_ICON, LIGHT, TILE],
  ['icon-maskable-512.png', 512, SAFE, LIGHT, FULL_GROUND],
  ['apple-touch-icon.png', 180, SAFE, LIGHT, FULL_GROUND],
  ['badge-96.png', 96, BADGE, MONO, null],
]
for (const [name, size, view, colors, ground] of icons) {
  const file = fileURLToPath(new URL(`../public/${name}`, import.meta.url))
  fs.writeFileSync(file, png(size, draw(size, view, colors, ground)))
  console.log(`wrote ${name}, ${size} px`)
}
