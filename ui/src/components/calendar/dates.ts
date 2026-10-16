import type { Occurrence } from './queries'

export type CalendarView = 'day' | 'week' | 'month'

export function dateKey(date: Date): string {
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

export function shiftDate(date: string, days: number): string {
  const value = new Date(`${date}T12:00:00Z`)
  value.setUTCDate(value.getUTCDate() + days)
  return value.toISOString().slice(0, 10)
}

export function midnight(date: string): number {
  return new Date(`${date}T00:00:00`).getTime()
}

export function calendarDays(date: string, view: CalendarView): string[] {
  const first = view === 'month' ? `${date.slice(0, 7)}-01` : date
  const weekday = new Date(`${first}T12:00:00`).getDay()
  const start = view === 'day' ? date : shiftDate(first, -((weekday + 6) % 7))
  const count = view === 'day' ? 1 : view === 'week' ? 7 : 42
  return Array.from({ length: count }, (_, i) => shiftDate(start, i))
}

export function moveDate(date: string, view: CalendarView, direction: number) {
  if (view !== 'month') {
    return shiftDate(date, direction * (view === 'day' ? 1 : 7))
  }
  const selected = new Date(`${date}T12:00:00`)
  return dateKey(new Date(selected.getFullYear(), selected.getMonth() + direction, 1))
}

export function dayLabel(day: string): string {
  return new Date(`${day}T12:00:00`).toLocaleDateString(undefined, {
    weekday: 'short',
    day: 'numeric',
  })
}

export function eventTime(occurrence: Occurrence): string {
  if (occurrence.event.data.all_day) return 'All day'
  return new Date(occurrence.start).toLocaleTimeString(undefined, {
    hour: 'numeric',
    minute: '2-digit',
  })
}

export function occurrencesForDay(items: Occurrence[], day: string) {
  const next = shiftDate(day, 1)
  return items.filter((item) => {
    if (item.event.data.all_day) return item.start < next && item.end > day
    return Date.parse(item.start) < midnight(next) && Date.parse(item.end) > midnight(day)
  })
}

export function minuteOfDay(value: string, day: string): number {
  const date = new Date(value)
  if (date.getTime() <= midnight(day)) return 0
  if (date.getTime() >= midnight(shiftDate(day, 1))) return 1440
  return date.getHours() * 60 + date.getMinutes() + date.getSeconds() / 60
}
