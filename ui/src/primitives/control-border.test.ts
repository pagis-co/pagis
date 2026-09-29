// The boundary of a control holds 3:1 on its ground (WCAG 1.4.11), so
// a field, a select trigger and an off switch draw it with
// `--border-control`. `tokens.test.ts` checks the ratio; this test
// checks that the controls read the token.

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const here = dirname(fileURLToPath(import.meta.url))

/** The declarations of the first rule with exactly this selector. */
function rule(sheet: string, selector: string): string {
  const css = readFileSync(join(here, sheet), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const match = new RegExp(`(?:^|})\\s*${escaped}\\s*\\{([^}]*)\\}`).exec(css)
  if (match === null) throw new Error(`${sheet} has no rule ${selector}`)
  return match[1]
}

const boundaries: [string, string, string][] = [
  ['input.css', '.ui-input', 'border: 1px solid var(--border-control)'],
  ['input.css', '.ui-input:hover:not(:disabled)', 'border-color: var(--border-control-hover)'],
  ['select.css', '.ui-select-trigger', 'border: 1px solid var(--border-control)'],
  ['select.css', '.ui-select-trigger:hover:not(:disabled)', 'border-color: var(--border-control-hover)'],
  ['switch.css', '.ui-switch-track', 'background: var(--border-control)'],
  // A screen that frames a bare field or draws a slider track draws a
  // control boundary too.
  ['../components/Composer.css', '.composer', 'border: 1px solid var(--border-control)'],
  ['../components/connection/connection.css', '.connection-chips', 'border: 1px solid var(--border-control)'],
  ['../blocks/call.css', '.call-scrubber-bar', 'background: var(--border-control)'],
  ['../blocks/call.css', '.call-scrubber-flat', 'background: var(--border-control)'],
]

describe('a control draws its boundary with the control border', () => {
  for (const [sheet, selector, declaration] of boundaries) {
    it(`${selector} in ${sheet}`, () => {
      expect(rule(sheet, selector)).toContain(declaration)
    })
  }
})

// A checkbox and a radio stay native (coverage.test.ts): the browser
// draws their boundary at 3:1 or more. Their checked fill takes the
// accent, so a selection reads in the product's one selection color.

describe('a native checkbox and radio take the accent', () => {
  it('sets accent-color from the token on the root', () => {
    const css = readFileSync(join(here, '..', 'styles.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
    const root = /(?:^|})\s*:root\s*\{([^}]*)\}/.exec(css)
    expect(root?.[1]).toContain('accent-color: var(--accent)')
  })
})
