import * as fs from 'node:fs'
import * as path from 'node:path'
import { fileURLToPath } from 'node:url'

// The setup and status pages use the design system of the Product App
// (docs/UI-DESIGN.md). The build copies its tokens, the Inter font and
// the Pagis mark from their one source into dist/design, which the app
// package holds, so the pages load with no network and hold no value
// that a person copied by hand. The argument names another directory.
const desktop = fileURLToPath(new URL('..', import.meta.url))
const output = process.argv[2] ?? path.join(desktop, 'dist', 'design')
const sources = {
  'tokens.css': '../ui/src/tokens.css',
  'inter.woff2': 'node_modules/@fontsource-variable/inter/files/inter-latin-wght-normal.woff2',
  'pagis-mark.svg': '../assets/brand/pagis-mark.svg',
  'pagis-mark-dark.svg': '../assets/brand/pagis-mark-dark.svg',
}

fs.mkdirSync(output, { recursive: true })
for (const [name, source] of Object.entries(sources)) {
  fs.copyFileSync(path.join(desktop, source), path.join(output, name))
}
