import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '../../api/client'
import { Button, Dialog, Input } from '../../primitives'
import { useCalendar, type Occurrence } from './queries'
import { calendarDays, dateKey, midnight, moveDate, shiftDate } from './dates'
import type { CalendarView } from './dates'
import { DayWeekGrid } from './DayWeekGrid'
import { MonthGrid } from './MonthGrid'
import { EventEditor } from './EventEditor'
import { useCurrentWorkspaceId, useWorkspaceAccess } from '../../workspace'
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

export function Calendar({ api, initialDate, eventId }: {
  api: ApiClient
  eventId?: string
  initialDate?: string
}) {
  const { canRespond } = useWorkspaceAccess()
  const workspaceId = useCurrentWorkspaceId()
  const [date, setDate] = useState(initialDate ?? dateKey(new Date()))
  const [view, setView] = useState<CalendarView>('week')
  const [editing, setEditing] = useState<Occurrence | 'new' | null>(null)
  const [linkClosed, setLinkClosed] = useState(false)
  const linked = useQuery({
    queryKey: ['workspace', workspaceId, 'calendar-event', eventId],
    enabled: eventId !== undefined && !linkClosed,
    queryFn: async () => {
      const { data, error } = await api.GET('/api/v1/calendar/events/{event_id}', { params: { path: { event_id: eventId! } } })
      if (data === undefined) throw error ?? new Error('Could not load Calendar Event.')
      return data
    },
  })
  const selected = editing ?? (!linkClosed && linked.data ? { event: linked.data, start: linked.data.data.start, end: linked.data.data.end, read_only: !canRespond, workspace_name: null } : null)
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
    setLinkClosed(true)
    setEditing(null)
  }

  return (
    <section className="calendar-place">
      {linked.isError && <p role="alert">Could not load Calendar Event.</p>}
      <header className="calendar-toolbar">
        <h2>
          Calendar
        </h2>
        {canRespond && <Button onClick={() => setEditing('new')}>
          New event
        </Button>}
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
        open={selected !== null}
        onOpenChange={(open) => {
          if (!open) closeEditor()
        }}
        title={selected === 'new' ? 'New event' : 'Edit event'}
        description="Times use the Event time zone. The end date of an all-day Event is exclusive."
      >
        {selected !== null && (
          <EventEditor
            key={selected === 'new' ? 'new' : `${selected.event.id}-${selected.start}`}
            api={api}
            date={date}
            event={selected === 'new' ? undefined : selected.event}
            occurrenceStart={selected === 'new' ? undefined : selected.start}
            readOnly={!canRespond || (selected !== 'new' && selected.read_only)}
            workspaceName={selected !== 'new' ? selected.workspace_name ?? undefined : undefined}
            memberInvitation={selected !== 'new' && selected.event.workspace_id !== workspaceId}
            onClose={closeEditor}
          />
        )}
      </Dialog>
    </section>
  )
}
