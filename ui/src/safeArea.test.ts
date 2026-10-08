// The safe area. The viewport of `index.html` is `viewport-fit=cover`,
// so on a phone the page draws under the notch, the rounded corners
// and the home indicator. The root and each fixed layer pad themselves
// by the safe tokens of `tokens.css`. The content in the flow of the
// page sits inside the root, so it stays inside the safe area too, and
// no other rule reads the tokens.

import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const srcDir = resolve(dirname(fileURLToPath(import.meta.url)))

function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, '')
}

function stylesheets(dir: string): { name: string; css: string }[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) return stylesheets(path)
    if (!entry.name.endsWith('.css')) return []
    return [{ name: relative(srcDir, path), css: stripComments(readFileSync(path, 'utf8')) }]
  })
}

/** The bodies of every block that opens with `prelude`, nested blocks
 * included. */
function blocks(css: string, prelude: string): string {
  const escaped = prelude.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const opening = new RegExp(`(?:^|\\n)\\s*${escaped}\\s*\\{`, 'g')
  const bodies = [...css.matchAll(opening)].map((match) => {
    const open = match.index + match[0].length - 1
    let depth = 0
    let end = open
    for (; end < css.length; end += 1) {
      if (css[end] === '{') depth += 1
      else if (css[end] === '}' && (depth -= 1) === 0) break
    }
    return css.slice(open + 1, end)
  })
  if (bodies.length === 0) throw new Error(`no block opens with ${prelude}`)
  return bodies.join('\n')
}

const sides = ['top', 'right', 'bottom', 'left'] as const
type Side = (typeof sides)[number]

/** The sides whose safe token a rule reads. */
function insets(body: string): Side[] {
  return sides.filter((side) => body.includes(`var(--safe-${side})`))
}

/** Each fixed layer: its stylesheet, the path of blocks to its rule,
 * and the edges of the screen it touches. */
const layers: { name: string; file: string; path: string[]; edges: Side[] }[] = [
  { name: 'the shell root', file: 'styles.css', path: ['.app'], edges: [...sides] },
  { name: 'the sign-in page', file: 'sign-in.css', path: ['.sign-in'], edges: [...sides] },
  { name: 'the phone shell', file: 'components/phone/phone.css', path: ['.phone-shell'], edges: ['right', 'left'] },
  { name: 'the phone nav bar', file: 'components/phone/phone.css', path: ['.nav-bar'], edges: ['top'] },
  { name: 'the tab bar', file: 'components/phone/phone.css', path: ['.tab-bar'], edges: ['bottom'] },
  { name: 'the form sheet', file: 'primitives/sheet.css', path: ['.ui-sheet'], edges: ['top', 'right', 'left'] },
  { name: 'the form sheet footer', file: 'primitives/sheet.css', path: ['.ui-sheet-footer'], edges: ['bottom'] },
  { name: 'the action sheet', file: 'primitives/sheet.css', path: ['.ui-action-sheet'], edges: ['right', 'bottom', 'left'] },
  {
    name: 'the expanded live screen',
    file: 'components/Computers.css',
    path: ['.screen-expanded'],
    edges: [...sides],
  },
  {
    name: 'the toast viewport',
    file: 'primitives/toast.css',
    path: ['.ui-toast-viewport'],
    edges: ['right', 'bottom'],
  },
]

describe('the root and each fixed layer stay inside the safe area', () => {
  for (const layer of layers) {
    it(`pads ${layer.name} by the inset of each edge it touches`, () => {
      const css = stripComments(readFileSync(join(srcDir, layer.file), 'utf8'))
      const body = layer.path.reduce(blocks, css)
      expect(insets(body)).toEqual(layer.edges)
    })
  }

  // In a phone browser `100vh` is the height with the browser bars
  // hidden, so the foot of a layer that reads it goes under the bars.
  it('sizes no layer by 100vh', () => {
    for (const sheet of stylesheets(srcDir)) {
      expect(
        sheet.css.match(/\b100vh\b/g) ?? [],
        `${sheet.name} reads 100vh; write 100dvh, the height of the visible page`,
      ).toEqual([])
    }
  })
})
