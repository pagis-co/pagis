import { minuteOfDay, occurrencesForDay } from './dates'
import type { Occurrence } from './queries'

export interface TimedPlacement {
  occurrence: Occurrence
  start: number
  end: number
  column: number
  columns: number
}

/** Each group of intersecting Events shares its available columns. */
export function timedPlacements(items: Occurrence[], day: string): TimedPlacement[] {
  const placements = occurrencesForDay(items, day)
    .filter((item) => !item.event.data.all_day)
    .map((occurrence) => ({
      occurrence,
      start: minuteOfDay(occurrence.start, day),
      end: minuteOfDay(occurrence.end, day),
      column: 0,
      columns: 1,
    }))
    .sort((a, b) => a.start - b.start || b.end - a.end)
  let group: TimedPlacement[] = []
  let ends: number[] = []
  let groupEnd = -1

  function finishGroup() {
    for (const item of group) item.columns = ends.length
    group = []
    ends = []
  }

  for (const item of placements) {
    if (item.start >= groupEnd) finishGroup()
    const free = ends.findIndex((end) => end <= item.start)
    item.column = free === -1 ? ends.length : free
    ends[item.column] = item.end
    group.push(item)
    groupEnd = Math.max(...ends)
  }
  finishGroup()
  return placements
}
