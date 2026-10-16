import { expect, it } from 'vitest'
import { zonedTime } from './zonedTime'

it('uses the Event time zone across DST changes', () => {
  expect(zonedTime('2026-03-08T00:00:00', 'America/New_York')).toBe(Date.parse('2026-03-08T05:00:00Z'))
  expect(zonedTime('2026-03-09T00:00:00', 'America/New_York')).toBe(Date.parse('2026-03-09T04:00:00Z'))
  expect(zonedTime('2026-11-01T00:00:00', 'America/New_York')).toBe(Date.parse('2026-11-01T04:00:00Z'))
  expect(zonedTime('2026-11-02T00:00:00', 'America/New_York')).toBe(Date.parse('2026-11-02T05:00:00Z'))
  expect(zonedTime('2026-03-08T09:00:00', 'Asia/Kolkata')).toBe(Date.parse('2026-03-08T03:30:00Z'))
})

it('refuses incomplete input and times that do not exist', () => {
  expect(zonedTime('', 'UTC')).toBeNaN()
  expect(zonedTime('2026-03-08T09:00:00', 'Invalid')).toBeNaN()
  expect(zonedTime('2026-03-08T02:30:00', 'America/New_York')).toBeNaN()
})
