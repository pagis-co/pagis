// The LogoMark is the Pagis mark: three desks that make a P. The files
// in `assets/brand` are its exported form, so these tests hold the
// component, the files and the tokens to one drawing and one palette.

import { render } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

import { LogoMark } from './logo-mark'

const here = dirname(fileURLToPath(import.meta.url))
const brandDir = resolve(here, '../../../assets/brand')

function brandFile(name: string): Element {
  const text = readFileSync(resolve(brandDir, name), 'utf8')
  return new DOMParser().parseFromString(text, 'image/svg+xml').documentElement
}

const geometry = ['x', 'y', 'width', 'height', 'rx', 'd']

/** The drawing of each shape, in drawing order, without its paint. */
function shapes(svg: Element): string[] {
  return [...svg.querySelectorAll('rect, path')].map((shape) =>
    [shape.tagName.toLowerCase(), ...geometry.map((name) => shape.getAttribute(name) ?? '')].join(' '),
  )
}

function fills(svg: Element): string[] {
  return [...svg.querySelectorAll('rect, path')].map((shape) => shape.getAttribute('fill') ?? '')
}

/** The values of one custom property in `tokens.css`, in file order:
 * the light theme first, then the two dark blocks. */
function tokenValues(name: string): string[] {
  const css = readFileSync(resolve(here, '../tokens.css'), 'utf8')
  return [...css.matchAll(new RegExp(`${name}:\\s*([^;]+);`, 'g'))].map((match) => match[1].trim())
}

describe('LogoMark', () => {
  it('is hidden from assistive technology, because the name stands beside it', () => {
    const { container } = render(<LogoMark />)
    expect(container.querySelector('svg')?.getAttribute('aria-hidden')).toBe('true')
  })

  it('draws the shapes of the exported mark', () => {
    const { container } = render(<LogoMark />)
    const drawn = container.querySelector('svg') as SVGSVGElement
    const exported = brandFile('pagis-mark.svg')

    expect(drawn.getAttribute('viewBox')).toBe(exported.getAttribute('viewBox'))
    expect(shapes(drawn)).toEqual(shapes(exported))
    for (const file of ['pagis-mark-dark.svg', 'pagis-mark-mono.svg', 'pagis-favicon.svg']) {
      expect(shapes(brandFile(file)), file).toEqual(shapes(exported))
    }
  })

  it('gives the favicon the margin of the full drawing square', () => {
    // The mark files crop to the desks. A browser tab draws its icon
    // edge to edge, so the favicon keeps the 20 unit margin of the
    // 160 unit square to sit at the size of other tab icons.
    expect(brandFile('pagis-favicon.svg').getAttribute('viewBox')).toBe('0 0 160 160')
  })

  it('paints the three desks from the logo tokens', () => {
    const { container } = render(<LogoMark />)
    const classes = [...container.querySelectorAll('rect, path')].map((shape) =>
      shape.getAttribute('class'),
    )
    expect(classes).toEqual(['ui-logo-mark-top', 'ui-logo-mark-bowl', 'ui-logo-mark-bottom'])
  })

  it('keeps the logo tokens equal to the exported files in both themes', () => {
    const names = ['--logo-top', '--logo-bowl', '--logo-bottom']
    const light = fills(brandFile('pagis-mark.svg'))
    const dark = fills(brandFile('pagis-mark-dark.svg'))

    names.forEach((name, index) => {
      const [lightValue, ...darkValues] = tokenValues(name)
      expect(lightValue, name).toBe(light[index])
      expect(darkValues, name).toEqual([dark[index], dark[index]])
    })
  })

  it('carries the screen class next to its own', () => {
    const { container } = render(<LogoMark className="sidebar-logo" />)
    expect(container.querySelector('svg')?.getAttribute('class')).toBe('ui-logo-mark sidebar-logo')
  })
})
