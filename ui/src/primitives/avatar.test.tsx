// The chosen face color takes priority over the stable id color.

import { render } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { Avatar, AvatarGroup, avatarInitial } from './avatar'

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
  it('draws the first letter of the owner name in upper case', () => {
    expect(avatarInitial('  sage')).toBe('S')
    expect(avatarInitial('')).toBe('?')
  })
})

describe('AvatarGroup', () => {
  it('overlaps the faces of a group in one span', () => {
    const { container } = render(
      <AvatarGroup>
        <Avatar id="agent-sage" name="Sage" size="sm" />
        <Avatar id="agent-clown" name="Clown" size="sm" />
      </AvatarGroup>,
    )
    const group = container.firstElementChild as HTMLElement
    expect(group.className).toContain('ui-avatar-group')
    expect(group.querySelectorAll('.ui-avatar')).toHaveLength(2)
  })
})
