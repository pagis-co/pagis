import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

import { stripComments, themes, value } from './test/tokens'

// The design tokens are the single source of every color, type size,
// radius and shadow (docs/UI-DESIGN.md). These tests read the
// stylesheets and fail on a literal that belongs in
// `tokens.css`, so no rule can add an ad-hoc value.

const styleDir = dirname(fileURLToPath(import.meta.url))
const tokensFile = 'tokens.css'

/** Every stylesheet under `src`, at any depth: the per-component sheets
 * beside the primitives are held to the same rule as `styles.css`. */
function stylesheetPaths(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) return stylesheetPaths(path)
    if (!entry.name.endsWith('.css') || entry.name === tokensFile) return []
    return [path]
  })
}

function stylesheets(): { name: string; css: string }[] {
  return stylesheetPaths(styleDir).map((path) => ({
    name: relative(styleDir, path),
    css: stripComments(readFileSync(path, 'utf8')),
  }))
}

function declarations(css: string, property: string): string[] {
  const pattern = new RegExp(`(?<![-\\w])${property}\\s*:([^;{}]*)`, 'g')
  return [...css.matchAll(pattern)].map((match) => match[1].trim())
}

// A value is token-clean when every part comes from a variable or is a
// keyword that carries no design decision.
const keywords = new Set([
  '0',
  'none',
  'inherit',
  'initial',
  'unset',
  'currentcolor',
  'transparent',
])

function offending(values: string[]): string[] {
  return values.filter((value) => {
    const stripped = value.replace(/var\(--[\w-]+\)/g, '').trim()
    if (stripped === '') return false
    return !stripped
      .split(/\s+/)
      .every((part) => keywords.has(part.toLowerCase()))
  })
}

const colorLiteral = /#[0-9a-fA-F]{3,8}\b|\b(?:rgba?|hsla?)\s*\(/g

describe('design tokens', () => {
  it('ships a tokens stylesheet', () => {
    expect(readdirSync(styleDir)).toContain(tokensFile)
  })

  for (const property of ['font-size', 'border-radius', 'box-shadow']) {
    it(`has no ${property} literal outside ${tokensFile}`, () => {
      for (const sheet of stylesheets()) {
        expect(
          offending(declarations(sheet.css, property)),
          `${sheet.name} sets ${property} outside the tokens`,
        ).toEqual([])
      }
    })
  }

  it(`has no hex or rgb color outside ${tokensFile}`, () => {
    for (const sheet of stylesheets()) {
      expect(
        sheet.css.match(colorLiteral) ?? [],
        `${sheet.name} names a color outside the tokens`,
      ).toEqual([])
    }
  })
})

// WCAG AA contrast (docs/UI-DESIGN.md, "every screen passes WCAG AA
// contrast in both themes"). The test reads the token values, so a
// change to a ramp step or a hue that puts a text pair under the
// threshold fails here and not in an audit.

function channels(color: string): [number, number, number] {
  const hex = /^#([0-9a-f]{6})$/i.exec(color)
  if (hex !== null) {
    const n = Number.parseInt(hex[1], 16)
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255]
  }
  const parts = /^rgba?\(([^)]+)\)$/.exec(color)
  if (parts === null) throw new Error(`cannot read the color ${color}`)
  const [r, g, b] = parts[1].split(',').map(Number)
  return [r, g, b]
}

/** Relative luminance, as WCAG 2.1 defines it. */
function luminance(color: string): number {
  const [r, g, b] = channels(color).map((part) => {
    const unit = part / 255
    return unit <= 0.03928 ? unit / 12.92 : ((unit + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

function contrast(text: string, surface: string): number {
  const a = luminance(text)
  const b = luminance(surface)
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)
}

/** Every ground a screen writes text on. */
const surfaces = [
  '--ground',
  '--panel',
  '--sidebar',
  '--sunken',
  '--hover',
  '--active',
  '--raised',
]

/** Every color a screen writes text in over one of those grounds. */
const inks = [
  '--text',
  '--muted',
  '--accent',
  '--failed',
  '--working',
  '--waiting',
  '--on-call',
  '--accent-ink',
  '--failed-ink',
  '--working-ink',
  '--on-call-ink',
  '--waiting-ink',
]

/** Text on a solid fill: the pairs that name their own ink. */
const fills: [string, string][] = [
  ['--on-accent', '--accent'],
  ['--on-accent', '--accent-ink'],
  ['--on-waiting', '--waiting'],
  ['--on-failed', '--failed'],
  ['--on-solid', '--stage'],
  ['--warm-1', '--warm-9'],
  ['--accent-ink', '--accent-soft'],
  ['--accent-ink', '--accent-tint'],
  ['--working-ink', '--working-soft'],
  ['--waiting-ink', '--waiting-soft'],
  ['--on-call-ink', '--on-call-soft'],
  ['--failed-ink', '--failed-soft'],
]

const AA = 4.5

describe('the text pairs pass WCAG AA', () => {
  for (const theme of themes()) {
    it(`holds ${AA}:1 on every ground in the ${theme.name} theme`, () => {
      const failures: string[] = []
      for (const surface of surfaces) {
        for (const ink of inks) {
          const ratio = contrast(value(theme.tokens, ink), value(theme.tokens, surface))
          if (ratio < AA) failures.push(`${ink} on ${surface} is ${ratio.toFixed(2)}:1`)
        }
      }
      expect(failures).toEqual([])
    })

    it(`holds ${AA}:1 on every solid fill in the ${theme.name} theme`, () => {
      const failures: string[] = []
      for (const [ink, fill] of fills) {
        const ratio = contrast(value(theme.tokens, ink), value(theme.tokens, fill))
        if (ratio < AA) failures.push(`${ink} on ${fill} is ${ratio.toFixed(2)}:1`)
      }
      expect(failures).toEqual([])
    })
  }
})

// WCAG 2.1 non-text contrast (1.4.11): the boundary that shows where a
// control is holds 3:1 on every ground a control sits on. A divider
// or a frame edge is decoration and keeps the lighter `--border`.

const NON_TEXT = 3

/** Every ground a control sits on. */
const controlGrounds = ['--ground', '--panel', '--sidebar', '--raised']

/** The marks that show a control: its boundary at rest and on hover. */
const controlBoundaries = ['--border-control', '--border-control-hover']

describe('the control boundaries pass WCAG non-text contrast', () => {
  for (const theme of themes()) {
    it(`holds ${NON_TEXT}:1 on every control ground in the ${theme.name} theme`, () => {
      const failures: string[] = []
      for (const ground of controlGrounds) {
        for (const boundary of controlBoundaries) {
          const ratio = contrast(value(theme.tokens, boundary), value(theme.tokens, ground))
          if (ratio < NON_TEXT) failures.push(`${boundary} on ${ground} is ${ratio.toFixed(2)}:1`)
        }
      }
      expect(failures).toEqual([])
    })

    it(`holds ${NON_TEXT}:1 between the switch knob and its off track in the ${theme.name} theme`, () => {
      const ratio = contrast(value(theme.tokens, '--raised'), value(theme.tokens, '--border-control'))
      expect(ratio).toBeGreaterThanOrEqual(NON_TEXT)
    })
  }
})

// The screens read the design system and nothing beside it. A screen
// that names a font stack, a ramp step, a shell width or a breakpoint
// of its own drifts from the system the day the system changes.

describe('the screens hold to the design system', () => {
  it('sets font-family only from a font token', () => {
    for (const sheet of stylesheets()) {
      const own = declarations(sheet.css, 'font-family').filter(
        (value) => !/^(?:var\(--font-(?:sans|serif|mono)\)|inherit)$/.test(value),
      )
      expect(own, `${sheet.name} names a font stack of its own`).toEqual([])
    }
  })

  it('reads a ramp step only through its alias', () => {
    for (const sheet of stylesheets()) {
      if (sheet.name.startsWith('primitives/')) continue
      expect(
        sheet.css.match(/var\(--warm-\d\)/g) ?? [],
        `${sheet.name} reads a ramp step instead of a surface or text alias`,
      ).toEqual([])
    }
  })

  it('reads a shell width from its token', () => {
    const widths = /(?<![\w.-])(?:280px|390px|680px|220px)\b/g
    for (const sheet of stylesheets()) {
      expect(
        sheet.css.match(widths) ?? [],
        `${sheet.name} writes a shell width instead of its token`,
      ).toEqual([])
    }
  })

  it('breaks the layout only at 760px and 460px', () => {
    for (const sheet of stylesheets()) {
      const widths = [...sheet.css.matchAll(/@media[^{]*\(max-width:\s*([^)]+)\)/g)].map(
        (match) => match[1].trim(),
      )
      expect(
        widths.filter((width) => width !== '760px' && width !== '460px'),
        `${sheet.name} breaks at a width of its own`,
      ).toEqual([])
    }
  })
})

// Weight, leading, tracking, opacity and spacing come from their scales
// in `tokens.css` too. A value may compose tokens in a calc(), negate
// one, or read the platform through env(); it names no length or
// number of its own.

/** The part of a value that is not a token, a composition of tokens
 * or a keyword. */
function ownValue(value: string, keywords: Set<string>): string {
  return value
    .replace(/env\([^)]*\)/g, '')
    .replace(/calc\(\s*-1\s*\*\s*var\(--[\w-]+\)\s*\)/g, '')
    .replace(/var\(--[\w-]+\)/g, '')
    .replace(/calc\(|\)|\s[+-]\s/g, ' ')
    .trim()
    .split(/\s+/)
    .filter((part) => part !== '' && part !== '0' && !keywords.has(part.toLowerCase()))
    .join(' ')
}

const scaled: [string, RegExp, Set<string>][] = [
  ['font-weight', /^font-weight$/, new Set(['inherit'])],
  ['line-height', /^line-height$/, new Set(['inherit', 'normal'])],
  ['letter-spacing', /^letter-spacing$/, new Set(['inherit', 'normal'])],
  ['opacity', /^opacity$/, new Set(['1'])],
  [
    'spacing',
    /^(?:padding|margin)(?:-(?:top|right|bottom|left|inline|block)(?:-(?:start|end))?)?$|^(?:row-|column-)?gap$/,
    new Set(['auto', 'inherit']),
  ],
]

describe('the scales hold every weight, leading, tracking, opacity and space', () => {
  for (const [name, property, keywords] of scaled) {
    it(`has no ${name} literal outside ${tokensFile}`, () => {
      const offenders: string[] = []
      for (const sheet of stylesheets()) {
        for (const match of sheet.css.matchAll(/(?<![-\w])([a-z-]+)\s*:\s*([^;{}]+)/g)) {
          if (!property.test(match[1])) continue
          const own = ownValue(match[2].trim(), keywords)
          if (own !== '') offenders.push(`${sheet.name}: ${match[1]}: ${match[2].trim()}`)
        }
      }
      expect(offenders).toEqual([])
    })
  }
})

// The safe area. The page draws under the notch, the rounded corners
// and the home indicator of a phone (`viewport-fit=cover` in
// `index.html`), so the root and each fixed layer pad themselves by
// these four insets. A screen that has no such parts gives 0px.

describe('the safe area', () => {
  it('names each inset of the screen, and 0px where the screen has none', () => {
    const [light] = themes()
    expect({
      top: light.tokens['--safe-top'],
      right: light.tokens['--safe-right'],
      bottom: light.tokens['--safe-bottom'],
      left: light.tokens['--safe-left'],
    }).toEqual({
      top: 'env(safe-area-inset-top, 0px)',
      right: 'env(safe-area-inset-right, 0px)',
      bottom: 'env(safe-area-inset-bottom, 0px)',
      left: 'env(safe-area-inset-left, 0px)',
    })
  })
})
