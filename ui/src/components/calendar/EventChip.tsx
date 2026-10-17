import { Button } from '../../primitives'
import { eventTime } from './dates'
import type { Occurrence } from './queries'

export function EventChip({ occurrence, onOpen }: {
  occurrence: Occurrence
  onOpen: (occurrence: Occurrence) => void
}) {
  const { title, meeting_url } = occurrence.event.data
  return (
    <>
      <Button
        size="sm"
        variant="ghost"
        className="calendar-event-open"
        onClick={() => onOpen(occurrence)}
      >
        <strong>
          {occurrence.event.data.invitation?.status === 'cancelled' ? `Cancelled: ${title}` : title}
        </strong>
        {occurrence.workspace_name && <span>{occurrence.workspace_name}</span>}
        <span>
          {eventTime(occurrence)}
        </span>
      </Button>
      {meeting_url && (
        <a
          href={meeting_url}
          target="_blank"
          rel="noopener noreferrer"
        >
          Join meeting
        </a>
      )}
    </>
  )
}
