import { dayLabel, occurrencesForDay } from './dates'
import { EventChip } from './EventChip'
import { timedPlacements } from './layout'
import type { Occurrence } from './queries'

const hours = Array.from({ length: 24 }, (_, hour) => hour)

export function DayWeekGrid({ days, occurrences, onOpen }: {
  days: string[]
  occurrences: Occurrence[]
  onOpen: (occurrence: Occurrence) => void
}) {
  const columns = `3rem repeat(${days.length}, minmax(0, 1fr))`
  const view = days.length === 1 ? 'day' : 'week'
  const allDay = occurrences.filter((item) => item.event.data.all_day)

  return (
    <div
      className="calendar-time-scroll"
      aria-label={`${view} view`}
    >
      <div className={`calendar-time-content calendar-time-${view}`}>
        <div
          className="calendar-time-head"
          style={{ gridTemplateColumns: columns }}
        >
          <span aria-hidden="true" />
          {days.map((day) => (
            <h3 key={day}>
              {dayLabel(day)}
            </h3>
          ))}
        </div>
        <section
          className="calendar-all-day"
          aria-label="All-day events"
          style={{ gridTemplateColumns: columns }}
        >
          <span>
            All day
          </span>
          {days.map((day) => (
            <div
              key={day}
              className="calendar-all-day-cell"
            >
              {occurrencesForDay(allDay, day).map((item) => (
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
            </div>
          ))}
        </section>
        <div
          className="calendar-time-body"
          style={{ gridTemplateColumns: columns }}
        >
          <div
            className="calendar-hour-axis"
            aria-label="Hours"
          >
            {hours.map((hour) => (
              <span key={hour}>
                {String(hour).padStart(2, '0')}:00
              </span>
            ))}
          </div>
          {days.map((day) => (
            <div
              key={day}
              className="calendar-timed-day"
              aria-label={day}
            >
              {timedPlacements(occurrences, day).map((item) => (
                <article
                  key={`${item.occurrence.event.id}-${item.occurrence.start}`}
                  aria-label={item.occurrence.event.data.title}
                  className="calendar-event calendar-timed-event"
                  style={{
                    top: `${item.start / 1440 * 100}%`,
                    height: `${(item.end - item.start) / 1440 * 100}%`,
                    left: `${item.column / item.columns * 100}%`,
                    width: `${100 / item.columns}%`,
                  }}
                >
                  <EventChip
                    occurrence={item.occurrence}
                    onOpen={onOpen}
                  />
                </article>
              ))}
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
