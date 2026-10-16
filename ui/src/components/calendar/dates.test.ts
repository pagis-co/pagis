import { expect, it } from 'vitest'
import { calendarDays, dateKey, midnight, moveDate, shiftDate } from './dates'

it('moves by calendar dates across month and year boundaries', () => {
  expect(shiftDate('2026-12-31', 1)).toBe('2027-01-01')
  expect(shiftDate('2024-02-28', 1)).toBe('2024-02-29')
  expect(moveDate('2026-01-31', 'month', 1)).toBe('2026-02-01')
  expect(moveDate('2026-03-09', 'week', -1)).toBe('2026-03-02')
})

it('uses Monday weeks and a complete month grid', () => {
  expect(calendarDays('2026-03-09', 'day')).toEqual(['2026-03-09'])
  const week = calendarDays('2026-03-11', 'week')
  expect(week).toHaveLength(7)
  expect(week[0]).toBe('2026-03-09')
  expect(week[6]).toBe('2026-03-15')
  const month = calendarDays('2026-03-09', 'month')
  expect(month).toHaveLength(42)
  expect(month[0]).toBe('2026-02-23')
})

it('uses local midnight rather than a fixed elapsed day', () => {
  for (const date of ['2026-03-08', '2026-11-01']) {
    expect(dateKey(new Date(midnight(date)))).toBe(date)
    expect(new Date(midnight(date)).getHours()).toBe(0)
    expect(dateKey(new Date(midnight(shiftDate(date, 1))))).toBe(shiftDate(date, 1))
  }
})
