/** Convert an Event wall time with the browser's IANA time zone data. */
export function zonedTime(value: string, timezone: string): number {
  const wall = Date.parse(`${value}Z`)
  if (!Number.isFinite(wall)) return NaN
  try {
    const formatter = new Intl.DateTimeFormat('en-CA', {
      timeZone: timezone, year: 'numeric', month: '2-digit', day: '2-digit',
      hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23',
    })
    let instant = wall
    for (let i = 0; i < 4; i++) {
      const parts = Object.fromEntries(formatter.formatToParts(instant).map(p => [p.type, p.value]))
      const rendered = Date.parse(`${parts.year}-${parts.month}-${parts.day}T${parts.hour}:${parts.minute}:${parts.second}Z`)
      const difference = wall - rendered
      if (difference === 0) return instant
      instant += difference
    }
  } catch { /* The editor can hold an incomplete time zone. */ }
  return NaN
}
