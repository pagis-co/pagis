import { render } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { AvatarGroup, OwnerAvatar, avatarInitial } from './avatar'

describe('avatarInitial', () => {
  it('draws the first letter of the owner name in upper case', () => {
    expect(avatarInitial('  sage')).toBe('S')
    expect(avatarInitial('')).toBe('?')
  })
})

describe('AvatarGroup', () => {
  it('overlaps the faces of a group in one span', () => {
    const { container } = render(
      <AvatarGroup>
        <OwnerAvatar name="Sage" size="sm" />
        <OwnerAvatar name="Clown" size="sm" />
      </AvatarGroup>,
    )
    const group = container.firstElementChild as HTMLElement
    expect(group.className).toContain('ui-avatar-group')
    expect(group.querySelectorAll('.ui-avatar')).toHaveLength(2)
  })
})
