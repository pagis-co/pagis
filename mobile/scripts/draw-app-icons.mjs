// Draw the Mobile App icons from the Pagis mark (assets/brand/README.md).
// Run `npm run icons` in mobile/ to make the PNG files again.

import { writeFileSync } from 'node:fs'
import { encode } from 'fast-png'
import { LIGHT, draw, png } from '../../assets/brand/mark.mjs'

const WHITE = [0xff, 0xff, 0xff]
const GROUND = { color: WHITE, covers: () => true }

// iOS rounds the icon itself. Use the layout of the web app touch icon,
// with a full white ground and no alpha channel, as the App Store needs.
const IOS = { x: -24, y: -24, size: 208 }

// Android shows the central 72 dp of a 108 dp adaptive layer. This view
// gives the mark a 48 dp box, inside the central 66 dp safe circle. The
// foreground has no ground: the launcher supplies it from the background
// layer, and cuts both layers to its own shape.
const ADAPTIVE = { x: -55, y: -55, size: 270 }

// The square and round icons show the same central 72 dp view.
const LAUNCHER = { x: -10, y: -10, size: 180 }
const ROUND = { color: WHITE, covers: (x, y) => (x - 80) ** 2 + (y - 80) ** 2 <= 90 ** 2 }

function write(path, size, view, ground, opaque = false) {
  const file = new URL(`../${path}`, import.meta.url)
  const pixels = draw(size, view, LIGHT, ground)
  const bytes = opaque
    ? encode({ width: size, height: size, channels: 3, data: pixels.filter((_, at) => at % 4 !== 3) })
    : png(size, pixels)
  writeFileSync(file, bytes)
  console.log(`wrote ${path}, ${size} px`)
}

write('ios/App/App/Assets.xcassets/AppIcon.appiconset/AppIcon-512@2x.png', 1024, IOS, GROUND, true)

for (const [density, size] of [['mdpi', 48], ['hdpi', 72], ['xhdpi', 96], ['xxhdpi', 144], ['xxxhdpi', 192]]) {
  const directory = `android/app/src/main/res/mipmap-${density}`
  write(`${directory}/ic_launcher.png`, size, LAUNCHER, GROUND)
  write(`${directory}/ic_launcher_round.png`, size, LAUNCHER, ROUND)
  write(`${directory}/ic_launcher_foreground.png`, size * 108 / 48, ADAPTIVE, null)
}
