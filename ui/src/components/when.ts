// The short moments a phone row says. A day near today is a word, and
// the time is the clock of the app (`formatClock`), so a row reads
// `tomorrow 07:30` and not a full locale timestamp.

import { dayKey, formatClock } from '../timeline'

const DAY = 86_400_000

function nearDay(at: number, now: number): string | null {
  const key = dayKey(at)
  if (key === dayKey(now)) return 'today'
  if (key === dayKey(now + DAY)) return 'tomorrow'
  if (key === dayKey(now - DAY)) return 'yesterday'
  return null
}

function calendarDay(at: number): string {
  return new Date(at).toLocaleDateString('en-US', { month: 'long', day: 'numeric' })
}

/** A moment before or after now: `today 05:00`, `tomorrow 07:30`,
 *  `yesterday 18:00`, else `November 8, 09:00`. */
export function nearMoment(at: number, now: number = Date.now()): string {
  const day = nearDay(at, now)
  return day !== null ? `${day} ${formatClock(at)}` : `${calendarDay(at)}, ${formatClock(at)}`
}

/** A past moment as short as it reads: `now` in the last minute, the
 *  clock earlier today, `yesterday`, else `September 14`. */
export function sinceWhen(at: number, now: number = Date.now()): string {
  if (now - at < 60_000) return 'now'
  const day = nearDay(at, now)
  if (day === 'today') return formatClock(at)
  if (day === 'yesterday') return day
  return calendarDay(at)
}
