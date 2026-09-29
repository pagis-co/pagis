// The Report and the work record on Home (ADR-0022).
//
// Home is the Report of the Chief of Staff. The heading says the day,
// the line under it says whose brief this is and when it was written,
// and the work record under the Report says what the office did. Every
// rule here is pure, so it is read on its own.

import type { RunDto } from '../../api/client'

/** The day, as Home writes it: `Tuesday, September 16`. */
export function homeDate(now: number): string {
  return new Date(now).toLocaleDateString(undefined, {
    weekday: 'long',
    month: 'long',
    day: 'numeric',
  })
}

/** The line under the date: whose brief this is, and when it was
 *  written. A Report that is being written now says so, because the
 *  prose below is still the one from before. */
export function briefLine(
  chiefName: string | null,
  writtenAt: number | null,
  writing = false,
): string {
  if (chiefName === null) return 'No sprite is the chief of staff'
  if (writing) return `${chiefName} is writing your brief`
  if (writtenAt === null) return `${chiefName} has written no brief yet`
  const clock = new Date(writtenAt).toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit',
  })
  return `${chiefName}’s brief · ${clock}`
}

/** One line of the work record: the Run, whether it is today's, and the
 *  stamp the row carries — a clock today, a date before. */
export interface RecordRow {
  run: RunDto
  today: boolean
  stamp: string
}

/** How many lines of the record Home shows. The rest is the Runs page. */
const RECORD_LINES = 12

function endedAt(run: RunDto): number {
  return run.ended_at ?? run.created_at
}

function sameDay(left: number, right: number): boolean {
  const day = new Date(left)
  const other = new Date(right)
  return (
    day.getFullYear() === other.getFullYear() &&
    day.getMonth() === other.getMonth() &&
    day.getDate() === other.getDate()
  )
}

/** The record Home calls `Work record · today and before`: the
 *  completed Runs, newest first, today's with a clock and the older
 *  ones with their date. */
export function workRecord(
  runs: readonly RunDto[],
  now: number = Date.now(),
): RecordRow[] {
  return runs
    .filter((run) => run.state === 'completed')
    .sort((left, right) => endedAt(right) - endedAt(left))
    .slice(0, RECORD_LINES)
    .map((run) => {
      const at = endedAt(run)
      const today = sameDay(at, now)
      return {
        run,
        today,
        stamp: today
          ? new Date(at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
          : new Date(at).toLocaleDateString(undefined, { month: 'short', day: 'numeric' }),
      }
    })
}
