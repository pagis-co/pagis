// The chosen face color takes priority over the stable id color.

import { render } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { Avatar } from './Avatar'

describe('Avatar', () => {
  it('shows the saved sprite and keeps the status ring', () => {
    const { container } = render(
      <Avatar
        id="sage"
        name="Sage"
        appearance={{
          sprite: 'pixie',
          preset: 'lavender',
          colors: {},
          accessories: {},
        }}
        presence="working"
      />,
    )
    expect(container.querySelector('img')?.src).toContain('lavender.png')
    expect(container.firstElementChild?.className).toContain(
      'ui-avatar-presence-working',
    )
  })
})
