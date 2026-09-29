// Every block sheet reads in both themes. The dark theme
// redefines only the tokens, so a card reads correctly there exactly
// when every color it draws comes from a token. This test reads all
// the stylesheets of the folder and fails on a color written by hand.

import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const here = dirname(fileURLToPath(import.meta.url))
const sheets = readdirSync(here)
  .filter((name) => name.endsWith('.css'))
  .sort()

function rules(name: string): string {
  return readFileSync(join(here, name), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
}

const colorProperties = [
  'color',
  'background',
  'background-color',
  'border',
  'border-color',
  'border-left',
  'box-shadow',
  'outline',
]

describe('the cards read in the dark theme', () => {
  it('finds the sheets of the blocks', () => {
    expect(sheets).toContain('card.css')
    expect(sheets.length).toBeGreaterThan(1)
  })

  for (const sheet of sheets) {
    it(`${sheet} names no color of its own`, () => {
      const css = rules(sheet)
      for (const property of colorProperties) {
        const pattern = new RegExp(`(?<![-\\w])${property}\\s*:([^;{}]*)`, 'g')
        for (const match of css.matchAll(pattern)) {
          const value = match[1]
          expect(
            /#[0-9a-f]{3,8}|rgba?\(|hsla?\(/i.test(value),
            `${sheet} writes the color ${value.trim()} instead of a token`,
          ).toBe(false)
        }
      }
    })
  }

  it('the failed call uses the failed hue and the live call the on-call hue', () => {
    const css = rules('call.css')
    expect(css).toContain('var(--failed-ink)')
    expect(css).toContain('var(--on-call)')
  })
})
