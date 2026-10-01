// Every screen renders through the primitives. These tests
// read the sources: a native control or a glyph icon in a screen
// fails here, so the primitives cannot quietly be bypassed.

import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const srcDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const primitivesDir = join(srcDir, 'primitives')

function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) return sources(path)
    if (!entry.name.endsWith('.tsx') || entry.name.endsWith('.test.tsx')) return []
    return [path]
  })
}

/** Every screen: the sources under `src`, less the primitives
 * themselves, which are the one place a native control is written. */
function screens(): { name: string; code: string }[] {
  return sources(srcDir)
    .filter((path) => !path.startsWith(primitivesDir))
    .map((path) => ({
      name: relative(srcDir, path),
      code: readFileSync(path, 'utf8'),
    }))
}

/** A file input, a checkbox and a radio stay native: no primitive
 * improves them, and the browser control is the accessible one. */
const nativeInput = /<input\b[^>]*type=(?:"|')(?:file|checkbox|radio)(?:"|')/

function inputTags(code: string): string[] {
  return [...code.matchAll(/<input\b[\s\S]*?\/?>/g)].map((match) => match[0])
}

describe('the screens render through the primitives', () => {
  for (const tag of ['button', 'select', 'textarea', 'datalist'] as const) {
    it(`writes no native <${tag}>`, () => {
      for (const screen of screens()) {
        expect(
          screen.code.includes(`<${tag}`),
          `${screen.name} writes a native <${tag}> instead of the primitive`,
        ).toBe(false)
      }
    })
  }

  it('writes a native <input> only for a file, a checkbox or a radio', () => {
    for (const screen of screens()) {
      for (const tag of inputTags(screen.code)) {
        expect(
          nativeInput.test(tag),
          `${screen.name} writes a native <input> instead of the Input primitive`,
        ).toBe(true)
      }
    }
  })
})

// Glyphs that stand in for icons. Each one draws differently on every
// operating system and reads as a placeholder.
const GLYPHS = [
  '◎',
  '▣',
  '≡',
  '↻',
  '❐',
  '⚙',
  '×',
  '✕',
  '☰',
  '▲',
  '▼',
  '✓',
  '🎤',
  '🔊',
  '🔈',
]

describe('the icons are Lucide and not glyphs', () => {
  it('has no glyph icon in a screen', () => {
    for (const screen of screens()) {
      for (const glyph of GLYPHS) {
        expect(
          screen.code.includes(glyph),
          `${screen.name} draws the glyph ${glyph}`,
        ).toBe(false)
      }
    }
  })

  it('has no lone plus as the content of an element', () => {
    for (const screen of screens()) {
      expect(
        />\s*\+\s*</.test(screen.code),
        `${screen.name} draws a plus glyph`,
      ).toBe(false)
    }
  })
})

// The primitives own their padding, border and radius. A screen
// class that sets one of them on an element a primitive already styles
// makes the primitive's own size and variant a lie, so the sweep below
// reads the sources, learns which class names reach a primitive, and
// fails on a rule that restyles the box of one.

/** The primitive components, from the barrel every screen imports. */
function primitiveNames(): string[] {
  const barrel = readFileSync(join(primitivesDir, 'index.ts'), 'utf8')
  return [
    ...new Set(
      [...barrel.matchAll(/^export \{ ([^}]+) \}/gm)]
        .flatMap((match) => match[1].split(','))
        .map((name) => name.trim())
        .filter((name) => /^[A-Z]/.test(name)),
    ),
  ]
}

/** A component that spreads its own props into a primitive passes the
 * className straight through, so its class names land on the primitive
 * too. */
function wrapperNames(components: string[]): string[] {
  const open = `<(?:${components.join('|')})\\b[^>]*\\{\\.\\.\\.\\w+\\}`
  return screens().flatMap((screen) =>
    new RegExp(open).test(screen.code)
      ? [...screen.code.matchAll(/export (?:function|const) ([A-Z]\w*)/g)].map(
          (match) => match[1],
        )
      : [],
  )
}

/** Every class name a screen puts on a primitive. */
function primitiveClasses(): Set<string> {
  const components = primitiveNames()
  const named = [...components, ...wrapperNames(components)]
  const opening = new RegExp(`<(?:${named.join('|')})\\b[\\s\\S]*?\\/?>`, 'g')
  const names = new Set<string>()
  for (const screen of screens()) {
    for (const [tag] of screen.code.matchAll(opening)) {
      for (const [, ...quoted] of tag.matchAll(
        /className=(?:"([^"]*)"|\{([\s\S]*?)\})/g,
      )) {
        // A template or a conditional names its classes in quotes or
        // backticks; the words between them are code, not classes.
        const literals = quoted.filter((part) => part !== undefined).join(' ')
        for (const word of literals.split(/[^\w-]+/)) {
          if (/^[a-z][\w-]*$/.test(word)) names.add(word)
        }
      }
    }
  }
  return names
}

function cssPaths(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) return cssPaths(path)
    return entry.name.endsWith('.css') ? [path] : []
  })
}

/** The properties that draw the box of a primitive. `border-collapse`
 * and `border-spacing` lay out a table and draw nothing. */
const boxProperty = /(?:^|[;\s])((?:padding|border)(?:-[a-z-]+)?)\s*:/g
const layoutOnly = new Set(['border-collapse', 'border-spacing'])

describe('the primitives own their padding, border and radius', () => {
  it('has no screen class that restyles the box of a primitive', () => {
    const classes = primitiveClasses()
    expect(classes.size).toBeGreaterThan(0)
    const offenders: string[] = []
    for (const path of cssPaths(srcDir)) {
      const css = readFileSync(path, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
      for (const [, selector, body] of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
        const hit = [...classes].find((name) =>
          new RegExp(`\\.${name}(?![\\w-])`).test(selector),
        )
        if (hit === undefined) continue
        for (const [, property] of body.matchAll(boxProperty)) {
          if (layoutOnly.has(property)) continue
          offenders.push(
            `${relative(srcDir, path)}: ${selector.trim()} sets ${property}`,
          )
        }
      }
    }
    expect(offenders).toEqual([])
  })
})

// Behaviour comes from Radix and Downshift through the primitives only, an icon
// takes one of the three control sizes, and a fixed look lives in a
// stylesheet, not in an inline style.

describe('the screens hold to the primitives', () => {
  it('imports no Radix or Downshift package', () => {
    for (const screen of screens()) {
      expect(
        /from '(?:@radix-ui\/|downshift')/.test(screen.code),
        `${screen.name} reaches Radix or Downshift around the primitives`,
      ).toBe(false)
    }
  })

  it('draws an icon at 14, 16 or 20 px', () => {
    for (const screen of screens()) {
      const sizes = [...screen.code.matchAll(/\bsize=\{(\d+)\}/g)].map((match) => match[1])
      expect(
        sizes.filter((size) => !['14', '16', '20'].includes(size)),
        `${screen.name} draws an icon off the scale`,
      ).toEqual([])
    }
  })

  it('writes an inline style only for a value computed at run time', () => {
    const fixed = /style=\{\{\s*\w+:\s*(?:-?[\d.]+|'[^']*'|"[^"]*")\s*(?:,\s*\w+:\s*(?:-?[\d.]+|'[^']*'|"[^"]*")\s*)*,?\s*\}\}/g
    for (const screen of screens()) {
      expect(
        screen.code.match(fixed) ?? [],
        `${screen.name} writes a fixed look inline`,
      ).toEqual([])
    }
  })
})
