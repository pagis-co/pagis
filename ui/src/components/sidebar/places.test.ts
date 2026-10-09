// The seven places of the sidebar and the place a path belongs to.

import { describe, expect, it } from 'vitest'

import { PLACES, placeForPath } from './places'

describe('the places', () => {
  it('lists the seven places in order', () => {
    expect(PLACES.map((place) => place.label)).toEqual([
      'Home',
      'Sprites',
      'Memory',
      'Automations',
      'Coding',
      'Software',
      'Settings',
    ])
  })

  it('reads the place from the path', () => {
    expect(placeForPath('/')).toBe('home')
    expect(placeForPath('/sprites')).toBe('sprites')
    // An Agent is one of the user's sprites.
    expect(placeForPath('/sprites/agent-1')).toBe('sprites')
    expect(placeForPath('/memory')).toBe('memory')
    expect(placeForPath('/automations')).toBe('automations')
    expect(placeForPath('/coding')).toBe('coding')
    expect(placeForPath('/coding/session-1')).toBe('coding')
    expect(placeForPath('/software')).toBe('software')
    expect(placeForPath('/settings/retention')).toBe('settings')
  })

  it('claims no place for a conversation or a run', () => {
    expect(placeForPath('/c/channel-1')).toBeNull()
    expect(placeForPath('/c/channel-1/t/message-1')).toBeNull()
    expect(placeForPath('/runs')).toBeNull()
  })
})
