import { useState } from 'react'
import type { ApiClient } from '../../api/client'
import { Button, Dialog, Input } from '../../primitives'
import { useCalendar, type Occurrence } from './queries'
import { calendarDays, dateKey, midnight, moveDate, shiftDate } from './dates'
import type { CalendarView } from './dates'
import { DayWeekGrid } from './DayWeekGrid'
import { MonthGrid } from './MonthGrid'
import { EventEditor } from './EventEditor'
import './calendar.css'

interface ViewOption {
  id: CalendarView
  label: string
}

const views: ViewOption[] = [
  { id: 'day', label: 'Day' },
  { id: 'week', label: 'Week' },
  { id: 'month', label: 'Month' },
]

export function Calendar({ api, initialDate }: {
  api: ApiClient
  initialDate?: string
}) {
  const [date, setDate] = useState(initialDate ?? dateKey(new Date()))
  const [view, setView] = useState<CalendarView>('week')
  const [editing, setEditing] = useState<Occurrence | 'new' | null>(null)
  const days = calendarDays(date, view)
  const from = midnight(days[0])
  const to = midnight(shiftDate(days[days.length - 1], 1))
  const events = useCalendar(api, from, to)
  const month = new Date(`${date}T12:00:00`).toLocaleDateString(undefined, {
    month: 'long',
    year: 'numeric',
  })
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone

  function openDay(day: string) {
    setDate(day)
    setView('day')
  }

  function closeEditor() {
    setEditing(null)
  }

  return (
    <section className="calendar-place">
      <header className="calendar-toolbar">
        <h2>
          Calendar
        </h2>
        <Button onClick={() => setEditing('new')}>
          New event
        </Button>
        <nav aria-label="Calendar views">
          {views.map((mode) => (
            <Button
              key={mode.id}
              aria-pressed={view === mode.id}
              onClick={() => setView(mode.id)}
            >
              {mode.label}
            </Button>
          ))}
        </nav>
        <div className="calendar-actions">
          <Button
            aria-label="Previous period"
            onClick={() => setDate(moveDate(date, view, -1))}
          >
            Previous
          </Button>
          <Button onClick={() => setDate(dateKey(new Date()))}>
            Today
          </Button>
          <Button
            aria-label="Next period"
            onClick={() => setDate(moveDate(date, view, 1))}
          >
            Next
          </Button>
          <Input
            aria-label="Calendar date"
            type="date"
            value={date}
            onChange={(event) => {
              if (event.target.value) setDate(event.target.value)
            }}
          />
        </div>
      </header>
      <p>
        {month} · Times in {zone}
      </p>
      {events.isError && <p role="alert">
        Could not load Calendar.
      </p>}
      {events.isPending && <p>
        Loading Calendar…
      </p>}
      {events.data && (view === 'month' ? (
        <MonthGrid
          days={days}
          occurrences={events.data}
          onOpen={setEditing}
          onMore={openDay}
        />
      ) : (
        <DayWeekGrid
          days={days}
          occurrences={events.data}
          onOpen={setEditing}
        />
      ))}
      {events.data?.length === 1000 && (
        <p>
          The range has reached its limit. Choose a shorter range.
        </p>
      )}
      <Dialog
        open={editing !== null}
        onOpenChange={(open) => {
          if (!open) closeEditor()
        }}
        title={editing === 'new' ? 'New event' : 'Edit event'}
        description="Times use the Event time zone. The end date of an all-day Event is exclusive."
      >
        {editing !== null && (
          <EventEditor
            key={editing === 'new' ? 'new' : `${editing.event.id}-${editing.start}`}
            api={api}
            date={date}
            event={editing === 'new' ? undefined : editing.event}
            occurrenceStart={editing === 'new' ? undefined : editing.start}
            onClose={closeEditor}
          />
        )}
      </Dialog>
    </section>
  )
}
