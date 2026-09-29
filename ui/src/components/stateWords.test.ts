// The words for the state of one Agent. The Activity says what the
// Agent does, the Computer state says whether its Computer runs, and no
// word is in both sets (CONTEXT.md).

import { describe, expect, it } from 'vitest'

import {
  ACTIVITIES,
  COMPUTER_STATES,
  activityTone,
  activityWord,
  computerStateWord,
  controlWord,
} from './stateWords'

describe('the Activity words', () => {
  it.each([
    ['working', 'Working'],
    ['waiting', 'Needs you'],
    ['oncall', 'On a call'],
    ['idle', 'Idle'],
  ] as const)('names %s as %s', (activity, word) => {
    expect(activityWord(activity)).toBe(word)
  })

  it('says nothing for a face with no Activity', () => {
    expect(activityWord('none')).toBe('')
  })

  it('gives each Activity the hue of its state', () => {
    expect(activityTone('working')).toBe('working')
    expect(activityTone('waiting')).toBe('waiting')
    expect(activityTone('oncall')).toBe('on-call')
    expect(activityTone('idle')).toBe('neutral')
  })
})

describe('the Computer state words', () => {
  it.each([
    ['off', 'Asleep'],
    ['starting', 'Starting…'],
    ['awake', 'Awake'],
    ['failed', 'Needs attention'],
  ])('names %s as %s', (state, word) => {
    expect(computerStateWord(state, null)).toBe(word)
  })

  it('counts the download of the Computer Image', () => {
    expect(computerStateWord('pulling', 40)).toBe('Downloading the computer… 40%')
  })

  // A new installation has the image and has not started the Computer.
  // It is off, so it is asleep: one state has one word.
  it('names every stopped Computer asleep', () => {
    expect(computerStateWord('off', null)).toBe('Asleep')
  })
})

describe('the two sets', () => {
  it('share no word', () => {
    const activity = ACTIVITIES.map((value) => activityWord(value).toLowerCase())
    const computer = COMPUTER_STATES.map((value) =>
      computerStateWord(value, 0).toLowerCase(),
    )
    for (const word of activity) {
      for (const other of computer) {
        expect(other.includes(word)).toBe(false)
        expect(word.includes(other)).toBe(false)
      }
    }
  })

  // Control of the screen is not work: the Agent can hold it and run
  // nothing.
  it('says who has control of the screen, not who works', () => {
    expect(controlWord('agent', 'Pixie')).toBe('Pixie is in control')
    expect(controlWord('user', 'Pixie')).toBe('You are in control')
    expect(controlWord('daemon', 'Pixie')).toBe('Pagis is filling a saved login')
    for (const holder of ['agent', 'user', 'daemon'] as const) {
      expect(controlWord(holder, 'Pixie').toLowerCase()).not.toContain('work')
    }
  })
})
