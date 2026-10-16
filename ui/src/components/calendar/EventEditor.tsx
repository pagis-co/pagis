import { useState, type FormEvent } from 'react'
import type { ApiClient } from '../../api/client'
import { Button, Combobox, Input, Select, Textarea } from '../../primitives'
import { InvitationActions } from './InvitationActions'
import { FreeBusy } from './FreeBusy'
import { errorMessage } from '../../queries'
import { AttendeesEditor } from './AttendeesEditor'
import { shiftDate } from './dates'
import { useDeleteEvent, useSaveEvent } from './queries'
import type { CalendarEvent, EventData } from './queries'

const repeatItems = [
  { label: 'Does not repeat', value: 'none' },
  { label: 'Daily', value: 'FREQ=DAILY' },
  { label: 'Weekly', value: 'FREQ=WEEKLY' },
  { label: 'Monthly', value: 'FREQ=MONTHLY' },
  { label: 'Yearly', value: 'FREQ=YEARLY' },
  { label: 'Custom', value: 'custom' },
]
const zones = [
  'UTC',
  'America/New_York',
  'America/Los_Angeles',
  'Europe/London',
  'Europe/Paris',
  'Asia/Kolkata',
  'Asia/Tokyo',
  'Australia/Sydney',
]

function repeatValue(rule: EventData['rrule']) {
  if (!rule) return 'none'
  return repeatItems.some((item) => item.value === rule) ? rule : 'custom'
}

function initialDraft(date: string): EventData {
  return {
    title: '',
    description: '',
    location: '',
    start: `${date}T09:00:00`,
    end: `${date}T10:00:00`,
    all_day: false,
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    rrule: null,
    exdates: [],
    attendees: [],
    reminders: [15],
    meeting_url: null,
  }
}

function inputTime(value: string) {
  return value.length === 16 ? `${value}:00` : value
}

export function EventEditor({ api, date, event, occurrenceStart, onClose }: {
  api: ApiClient
  date: string
  event?: CalendarEvent
  occurrenceStart?: string
  onClose: () => void
}) {
  const [draft, setDraft] = useState<EventData>(() => event?.data ?? initialDraft(date))
  const [repeat, setRepeat] = useState(repeatValue(draft.rrule))
  const [reminders, setReminders] = useState(draft.reminders.join(', '))
  const [validation, setValidation] = useState('')
  const save = useSaveEvent(api, event)
  const remove = useDeleteEvent(api)
  const busy = save.isPending || remove.isPending
  const failure = validation || (
    save.isError || remove.isError
      ? errorMessage(remove.error ?? save.error, 'Could not save the change. Check the times, time zone and recurrence rule.')
      : ''
  )

  function change<K extends keyof EventData>(key: K, value: EventData[K]) {
    setDraft((old) => ({ ...old, [key]: value }))
  }

  function saveBody(body: EventData) {
    save.mutate(body, { onSuccess: onClose })
  }

  function submit(e: FormEvent) {
    e.preventDefault()
    const values = reminders.trim()
      ? reminders.split(',').map((value) => Number(value.trim()))
      : []
    const invalid = values.some((value) => (
      !Number.isInteger(value) || value < 0 || value > 4294967295
    ))
    if (invalid) {
      setValidation('Use whole numbers of minutes, separated by commas.')
      return
    }
    setValidation('')
    saveBody({ ...draft, reminders: values })
  }

  function toggleAllDay(allDay: boolean) {
    setDraft((old) => ({
      ...old,
      all_day: allDay,
      start: allDay ? old.start.slice(0, 10) : `${old.start}T09:00:00`,
      end: allDay
        ? shiftDate(old.start.slice(0, 10), 1)
        : `${old.end}T10:00:00`,
      exdates: [],
    }))
  }

  function changeRepeat(value: string) {
    setRepeat(value)
    if (value === 'none') change('rrule', null)
    else if (value === 'custom') change('rrule', draft.rrule ?? 'FREQ=WEEKLY')
    else change('rrule', value)
  }

  function deleteEvent() {
    if (event) remove.mutate(event.id, { onSuccess: onClose })
  }

  function deleteOccurrence() {
    if (!event || !occurrenceStart) return
    const length = event.data.all_day ? 10 : 19
    saveBody({
      ...event.data,
      exdates: [...event.data.exdates, occurrenceStart.slice(0, length)],
    })
  }

  if (event?.created_by.kind === 'system' && event.data.invitation) {
    return <section className="calendar-editor" aria-label="Invitation details">
      <h3>{event.data.title}</h3>
      <p>{event.data.start} – {event.data.end} ({event.data.timezone})</p>
      <p>{event.data.location}</p>
      <p>{event.data.description}</p>
      <InvitationActions api={api} event={event} />
      <Button type="button" onClick={onClose}>Close</Button>
    </section>
  }

  return (
    <form
      className="calendar-editor"
      aria-label="Event editor"
      onSubmit={submit}
    >
      {event?.data.invitation && <InvitationActions api={api} event={event} />}
      {event?.data.rrule && <p>
        Changes apply to the complete series.
      </p>}
      <label>
        Title
        <Input
          required
          maxLength={1024}
          value={draft.title}
          onChange={(e) => change('title', e.target.value)}
        />
      </label>
      <label>
        Description
        <Textarea
          value={draft.description}
          onChange={(e) => change('description', e.target.value)}
        />
      </label>
      <label>
        Location
        <Input
          value={draft.location}
          onChange={(e) => change('location', e.target.value)}
        />
      </label>
      <label className="calendar-checkbox">
        <input
          type="checkbox"
          checked={draft.all_day}
          onChange={(e) => toggleAllDay(e.target.checked)}
        />
        All day
      </label>
      <div className="calendar-editor-times">
        <label>
          Start
          <Input
            required
            type={draft.all_day ? 'date' : 'datetime-local'}
            step={draft.all_day ? undefined : 1}
            value={draft.start}
            onChange={(e) => change('start', inputTime(e.target.value))}
          />
        </label>
        <label>
          End {draft.all_day ? '(exclusive)' : ''}
          <Input
            required
            type={draft.all_day ? 'date' : 'datetime-local'}
            step={draft.all_day ? undefined : 1}
            value={draft.end}
            onChange={(e) => change('end', inputTime(e.target.value))}
          />
        </label>
      </div>
      <label>
        Time zone
        <Combobox
          required
          label="Time zone"
          value={draft.timezone}
          onValueChange={(value) => change('timezone', value)}
          items={zones}
        />
      </label>
      <label>
        Repeat
        <Select
          label="Repeat"
          value={repeat}
          onValueChange={changeRepeat}
          items={repeatItems}
        />
      </label>
      {repeat === 'custom' && (
        <label>
          RRULE
          <Input
            required
            value={draft.rrule ?? ''}
            onChange={(e) => change('rrule', e.target.value)}
          />
        </label>
      )}
      <AttendeesEditor
        attendees={draft.attendees}
        onChange={(attendees) => change('attendees', attendees)}
      />
      <FreeBusy api={api} draft={draft} />
      <label>
        Reminder minutes
        <Input
          value={reminders}
          onChange={(e) => setReminders(e.target.value)}
        />
      </label>
      <p className="calendar-hint">
        Minutes before the start, separated by commas. Reminders are saved.
        Notification delivery is not built.
      </p>
      <label>
        Meeting link
        <Input
          type="url"
          pattern="https://.*"
          value={draft.meeting_url ?? ''}
          onChange={(e) => change('meeting_url', e.target.value || null)}
        />
      </label>
      {failure && <p role="alert">
        {failure}
      </p>}
      <div className="calendar-actions">
        <Button
          type="submit"
          disabled={busy}
        >
          Save event
        </Button>
        <Button
          type="button"
          onClick={onClose}
        >
          Cancel
        </Button>
        {event && (
          <Button
            type="button"
            disabled={busy}
            onClick={deleteEvent}
          >
            {draft.rrule ? 'Delete series' : 'Delete event'}
          </Button>
        )}
        {event?.data.rrule && occurrenceStart && (
          <Button
            type="button"
            disabled={busy}
            onClick={deleteOccurrence}
          >
            Delete this occurrence
          </Button>
        )}
      </div>
    </form>
  )
}
