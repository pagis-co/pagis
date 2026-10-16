import { Button, Input, Select } from '../../primitives'
import type { EventData } from './queries'

type Attendee = EventData['attendees'][number]
const statuses: Attendee['status'][] = [
  'needs-action',
  'accepted',
  'declined',
  'tentative',
]
const statusItems = statuses.map((value) => ({ label: value, value }))

export function AttendeesEditor({ attendees, onChange }: {
  attendees: Attendee[]
  onChange: (attendees: Attendee[]) => void
}) {
  function update(index: number, changes: Partial<Attendee>) {
    onChange(attendees.map((attendee, i) => (
      i === index ? { ...attendee, ...changes } : attendee
    )))
  }

  function add() {
    onChange([...attendees, { email: '', name: '', status: 'needs-action' }])
  }

  return (
    <fieldset>
      <legend>
        Attendees
      </legend>
      {attendees.map((attendee, index) => (
        <div
          className="calendar-attendee"
          key={index}
        >
          <Input
            aria-label={`Attendee email ${index + 1}`}
            type="email"
            required
            value={attendee.email}
            onChange={(event) => update(index, { email: event.target.value })}
          />
          <Input
            aria-label={`Attendee name ${index + 1}`}
            placeholder="Name"
            value={attendee.name}
            onChange={(event) => update(index, { name: event.target.value })}
          />
          <Select
            label={`Participation status ${index + 1}`}
            value={attendee.status}
            items={statusItems}
            onValueChange={(value) => update(index, { status: value as Attendee['status'] })}
          />
          <Button
            type="button"
            onClick={() => onChange(attendees.filter((_, i) => i !== index))}
          >
            Remove attendee {index + 1}
          </Button>
        </div>
      ))}
      <Button
        type="button"
        onClick={add}
      >
        Add attendee
      </Button>
    </fieldset>
  )
}
