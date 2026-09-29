// The frame of the first run. The Client App window has its own frame,
// so the steps sit on the ground of the window, with no panel, border,
// radius or shadow of their own. One step fills the window: the stepper
// at the top, the content of the step in the middle and its actions in a
// footer at the bottom, as the first run of Linear or Raycast does. The
// page never scrolls; only the middle scrolls when the window is too
// small for the content. The same page reads well in a browser tab.

import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const here = dirname(fileURLToPath(import.meta.url))

function stylesheet(path: string): string {
  return readFileSync(resolve(here, path), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
}

const css = stylesheet('onboarding.css')

/** The declarations of the one rule whose selector is `selector`. */
function rule(selector: string, sheet = css): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const match = new RegExp(`(?:^|\\})\\s*${escaped}\\s*\\{([^}]*)\\}`).exec(sheet)
  if (!match) throw new Error(`the stylesheet has no rule for ${selector}`)
  return match[1]
}

function declares(declarations: string, property: string): boolean {
  return new RegExp(`(?<![-\\w])${property}\\s*:`).test(declarations)
}

function value(declarations: string, property: string): string | undefined {
  return new RegExp(`(?<![-\\w])${property}\\s*:\\s*([^;]+)`).exec(declarations)?.[1].trim()
}

describe('the frame of the first run', () => {
  it('draws no card around the steps', () => {
    for (const selector of ['.onboarding', '.onboarding-column', '.onboarding-body']) {
      const declarations = rule(selector)
      for (const property of ['border', 'border-radius', 'box-shadow']) {
        expect(declares(declarations, property), `${selector} ${property}`).toBe(false)
      }
    }
    expect(value(rule('.onboarding'), 'background')).toBe('var(--ground)')
  })

  it('keeps the stepper, the content and the actions in one column of a readable width', () => {
    const column = rule('.onboarding-stepper,\n.onboarding-content,\n.onboarding-actions')
    expect(value(column, 'width')).toBe('min(44rem, 100% - 2 * var(--space-4))')
    expect(value(column, 'margin-inline')).toBe('auto')
  })

  // On macOS the window buttons sit over the top of the page. A strip as
  // high as their area moves the window and keeps the steps under it. A
  // browser defines no title bar area, so the strip has no height there.
  it('gives the window buttons their area and a drag region at the top', () => {
    const titlebar = rule('.onboarding-titlebar')
    expect(value(titlebar, 'height')).toBe('env(titlebar-area-height, 0px)')
    expect(value(titlebar, '-webkit-app-region')).toBe('drag')
    expect(value(titlebar, 'flex')).toBe('none')
  })
})

describe('one step fills the window', () => {
  // `height: 100%` fills the window only when every box above it has a
  // height. The page is the first child of `#root`.
  it('takes the height of the window, which the document passes down', () => {
    const root = rule('html,\nbody,\n#root', stylesheet('../../styles.css'))
    expect(value(root, 'height')).toBe('100%')
    const page = rule('.onboarding')
    expect(value(page, 'height')).toBe('100%')
    expect(value(page, 'display')).toBe('flex')
    expect(value(page, 'flex-direction')).toBe('column')
  })

  it('never scrolls the page', () => {
    expect(value(rule('.onboarding'), 'overflow')).toBe('hidden')
    const column = rule('.onboarding-column')
    expect(value(column, 'flex')).toBe('1')
    expect(value(column, 'min-height')).toBe('0')
    expect(value(column, 'display')).toBe('flex')
    expect(value(column, 'flex-direction')).toBe('column')
    expect(declares(column, 'overflow')).toBe(false)
  })

  // The middle takes the height the stepper and the footer leave, and
  // scrolls when its content is taller. `min-height: 0` lets it shrink
  // below its content, so the footer never leaves the window.
  it('scrolls the content of the step in the middle only', () => {
    const body = rule('.onboarding-body')
    expect(value(body, 'flex')).toBe('1')
    expect(value(body, 'min-height')).toBe('0')
    expect(value(body, 'overflow')).toBe('hidden auto')
  })

  it('pins the stepper at the top and the footer at the bottom', () => {
    expect(value(rule('.onboarding-stepper'), 'flex')).toBe('none')
    const footer = rule('.onboarding-actions')
    expect(value(footer, 'flex')).toBe('none')
    expect(value(footer, 'border-top')).toBe('1px solid var(--border)')
    expect(declares(footer, 'position')).toBe(false)
  })

  // Back sits on the left and the primary action on the right, also on
  // the first step, which has no Back.
  it('puts Back on the left and the primary action on the right', () => {
    expect(value(rule('.onboarding-actions'), 'justify-content')).toBe('flex-end')
    expect(value(rule('.onboarding-back'), 'margin-inline-end')).toBe('auto')
  })
})
