import { Button } from '../../primitives'
import { dayLabel, occurrencesForDay } from './dates'
import { EventChip } from './EventChip'
import type { Occurrence } from './queries'

const visibleCount = 3

export function MonthGrid({ days, occurrences, onOpen, onMore }: {
  days: string[]
  occurrences: Occurrence[]
  onOpen: (occurrence: Occurrence) => void
  onMore: (day: string) => void
}) {
  return (
    <div
      className="calendar-month"
      aria-label="month view"
    >
      {days.map((day) => {
        const items = occurrencesForDay(occurrences, day)
        const remaining = items.length - visibleCount
        return (
          <section
            className="calendar-day-cell"
            key={day}
            aria-label={day}
          >
            <h3>
              {dayLabel(day)}
            </h3>
            {items.slice(0, visibleCount).map((item) => (
              <article
                className="calendar-event"
                key={`${item.event.id}-${item.start}`}
              >
                <EventChip
                  occurrence={item}
                  onOpen={onOpen}
                />
              </article>
            ))}
            {remaining > 0 && (
              <Button
                size="sm"
                variant="ghost"
                className="calendar-more"
                onClick={() => onMore(day)}
              >
                +{remaining} more
              </Button>
            )}
          </section>
        )
      })}
    </div>
  )
}
