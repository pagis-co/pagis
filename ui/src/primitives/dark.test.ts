// Every primitive reads in the dark theme. The dark theme
// redefines only the tokens, so a primitive reads correctly there
// exactly when every color it draws comes from a token. This test reads
// every stylesheet beside the primitives and fails on a color written
// by hand.

import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const here = dirname(fileURLToPath(import.meta.url))
const sheets = readdirSync(here).filter((name) => name.endsWith('.css'))

function rules(name: string): string {
  return readFileSync(join(here, name), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
}

const colorProperties = [
  'color',
  'background',
  'background-color',
  'border',
  'border-color',
  'border-top',
  'border-right',
  'border-bottom',
  'border-left',
  'box-shadow',
  'outline',
]

describe('the primitives read in the dark theme', () => {
  it('finds the stylesheets of the primitives', () => {
    expect(sheets).toEqual(
      expect.arrayContaining(['frame.css', 'section-label.css', 'layout.css']),
    )
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
})
