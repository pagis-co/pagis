// The short moments a phone row says: a day near today in words, and
// the clock of the app.

import { describe, expect, it } from 'vitest'

import { formatClock } from '../timeline'
import { nearMoment, sinceWhen } from './when'

const now = new Date(2026, 9, 8, 12, 0).getTime()
const at = (day: number, hour: number, minute = 0) =>
  new Date(2026, 9, day, hour, minute).getTime()

describe('nearMoment', () => {
  it('names today, tomorrow and yesterday, with the clock of the app', () => {
    expect(nearMoment(at(8, 5), now)).toBe(`today ${formatClock(at(8, 5))}`)
    expect(nearMoment(at(9, 7, 30), now)).toBe(`tomorrow ${formatClock(at(9, 7, 30))}`)
    expect(nearMoment(at(7, 18), now)).toBe(`yesterday ${formatClock(at(7, 18))}`)
  })

  it('names the date of another day', () => {
    expect(nearMoment(new Date(2026, 10, 8, 9).getTime(), now)).toBe(
      `November 8, ${formatClock(new Date(2026, 10, 8, 9).getTime())}`,
    )
  })
})

describe('sinceWhen', () => {
  it('says now for the last minute, and the clock for earlier today', () => {
    expect(sinceWhen(now - 30_000, now)).toBe('now')
    expect(sinceWhen(at(8, 8, 55), now)).toBe(formatClock(at(8, 8, 55)))
  })

  it('says yesterday, and the date of an older day', () => {
    expect(sinceWhen(at(7, 23), now)).toBe('yesterday')
    expect(sinceWhen(new Date(2026, 8, 14, 10).getTime(), now)).toBe('September 14')
  })
})
