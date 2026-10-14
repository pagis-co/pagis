import { render } from '@testing-library/react'
import { expect, it } from 'vitest'
import { InitialTile } from './initial-tile'

it('draws the first letter in a tile and hides it from the accessible name', () => {
  const { container } = render(<InitialTile name="  acme" />)
  const tile = container.firstElementChild
  expect(tile?.textContent).toBe('A')
  expect(tile?.className).toBe('ui-initial-tile')
  expect(tile?.getAttribute('aria-hidden')).toBe('true')
})
