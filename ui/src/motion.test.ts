// Motion (docs/UI-DESIGN.md). Every transition and every animation
// reads a duration token, and the reduced-motion block
// sets each of those tokens to zero. Together the two rules mean that a
// reader who asks for less motion gets none: no rule can hold a
// duration of its own that the media query cannot reach.

import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const styleDir = dirname(fileURLToPath(import.meta.url))
const tokensFile = 'tokens.css'

function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, '')
}

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

/** A time the rule names itself instead of reading a token. */
const timeLiteral = /(?<![\w-])\d+(?:\.\d+)?m?s(?![\w-])/g

const motionProperties = [
  'transition',
  'transition-duration',
  'transition-delay',
  'animation',
  'animation-duration',
  'animation-delay',
]

/** The duration tokens `tokens.css` declares on `:root`. */
function durationTokens(css: string): string[] {
  return [...css.matchAll(/--duration-[\w-]+/g)].map((match) => match[0])
}

/** The body of the `prefers-reduced-motion: reduce` block. */
function reducedMotionBlock(css: string): string {
  const start = css.indexOf('@media (prefers-reduced-motion: reduce)')
  expect(start, 'tokens.css has no reduced-motion block').toBeGreaterThan(-1)
  return css.slice(start, css.indexOf('\n}\n', start))
}

describe('motion', () => {
  const tokens = stripComments(readFileSync(join(styleDir, tokensFile), 'utf8'))

  it('reduced motion zeroes every duration token', () => {
    const block = reducedMotionBlock(tokens)
    const declared = new Set(durationTokens(tokens))
    expect(declared.size).toBeGreaterThan(0)
    for (const token of declared) {
      expect(
        declarations(block, token),
        `${token} keeps its duration under reduced motion`,
      ).toEqual(['0ms'])
    }
  })

  for (const property of motionProperties) {
    it(`has no ${property} time outside ${tokensFile}`, () => {
      for (const sheet of stylesheets()) {
        for (const value of declarations(sheet.css, property)) {
          expect(
            value.match(timeLiteral) ?? [],
            `${sheet.name} names a ${property} time outside the tokens`,
          ).toEqual([])
        }
      }
    })
  }
})
