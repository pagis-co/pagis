import { fireEvent, render, screen, within } from '@testing-library/react'
import { expect, it, vi } from 'vitest'
import { calendarDays } from './dates'
import { DayWeekGrid } from './DayWeekGrid'
import { MonthGrid } from './MonthGrid'
import type { Occurrence } from './queries'

function occurrence(title: string, startHour: number, endHour: number): Occurrence {
  const start = new Date(2026, 2, 9, startHour).toISOString()
  const end = new Date(2026, 2, 9, endHour).toISOString()
  return {
    read_only: false,
    workspace_name: null,
    start,
    end,
    event: {
      id: title,
      uid: title,
      workspace_id: 'workspace-1',
      created_by: { kind: 'system' },
      data: {
        title, start, end, all_day: false, timezone: 'UTC',
        description: '', location: '', rrule: null, exdates: [],
        attendees: [], reminders: [], meeting_url: null,
      },
    },
  }
}

for (const days of [['2026-03-09'], calendarDays('2026-03-09', 'week')]) {
  it(`places timed events against an hour axis in ${days.length === 1 ? 'day' : 'week'}`, () => {
    const onOpen = vi.fn()
    render(
      <DayWeekGrid
        days={days}
        occurrences={[occurrence('Review', 9, 10), occurrence('Overlap', 9, 11)]}
        onOpen={onOpen}
      />,
    )
    expect(screen.getByText('09:00')).toBeTruthy()
    const review = screen.getByRole('article', { name: 'Review' })
    const overlap = screen.getByRole('article', { name: 'Overlap' })
    expect(review.style.top).toBe('37.5%')
    expect(parseFloat(review.style.height)).toBeCloseTo(100 / 24)
    expect(review.style.width).toBe('50%')
    expect(review.style.left).not.toBe(overlap.style.left)
    fireEvent.click(within(review).getByRole('button'))
    expect(onOpen).toHaveBeenCalled()
  })
}

it('keeps all-day events above the time grid', () => {
  const allDay = occurrence('Holiday', 0, 1)
  allDay.event.data.all_day = true
  allDay.start = '2026-03-09'
  allDay.end = '2026-03-10'
  render(
    <DayWeekGrid days={['2026-03-09']} occurrences={[allDay]} onOpen={vi.fn()} />,
  )
  const row = screen.getByRole('region', { name: 'All-day events' })
  expect(within(row).getByRole('button', { name: /Holiday/ })).toBeTruthy()
})

it('limits month chips and opens the day with more events', () => {
  const onMore = vi.fn()
  const items = Array.from({ length: 5 }, (_, i) => occurrence(`Event ${i}`, 9, 10))
  render(
    <MonthGrid days={['2026-03-09']} occurrences={items} onOpen={vi.fn()} onMore={onMore} />,
  )
  expect(screen.getAllByRole('article')).toHaveLength(3)
  fireEvent.click(screen.getByRole('button', { name: '+2 more' }))
  expect(onMore).toHaveBeenCalledWith('2026-03-09')
})
