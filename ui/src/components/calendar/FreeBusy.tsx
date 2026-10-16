import { useUser } from '../../queries'
import type { ApiClient } from '../../api/client'
import { shiftDate } from './dates'
import { useFreeBusy, type EventData } from './queries'
import { zonedTime } from './zonedTime'

export function FreeBusy({ api, draft }: { api: ApiClient; draft: EventData }) {
  const person = useUser(api)
  const day = draft.start.slice(0, 10)
  const from = zonedTime(`${day}T00:00:00`, draft.timezone)
  const to = Number.isFinite(from) ? zonedTime(`${shiftDate(day, 1)}T00:00:00`, draft.timezone) : NaN
  const addresses = [...new Set([person.data?.email ?? '', ...draft.attendees.map(a => a.email)]
    .map(address => address.trim().toLowerCase()).filter(Boolean))]
  const availability = useFreeBusy(api, from, to, addresses)
  const start = zonedTime(draft.start, draft.timezone)
  const end = zonedTime(draft.end, draft.timezone)
  const chosen = !draft.all_day && end > start
  const position = (start: number, end: number) => ({
    left: `${100 * (Math.max(from, start) - from) / (to - from)}%`,
    width: `${100 * (Math.min(to, end) - Math.max(from, start)) / (to - from)}%`,
  })
  const hours = Number.isFinite(from) && Number.isFinite(to)
    ? Array.from({ length: Math.ceil((to - from) / 3_600_000) }, (_, i) => from + i * 3_600_000)
    : []
  return <section aria-label="Free/Busy" className="calendar-free-busy">
    <h3>Free/Busy</h3>
    <p>{day} · {draft.timezone}. Shaded times are busy. The outline marks the chosen time. All-day Events are free.</p>
    {person.isError || availability.isError ? <p role="alert">Could not load Free/Busy.</p>
      : hours.length === 0 ? <p>Enter a valid day and time zone.</p>
      : availability.isPending ? <p>Loading Free/Busy…</p>
      : <div className="calendar-free-busy-scroll" tabIndex={0} role="region" aria-label="Attendee times">
        <table>
          <thead><tr><th scope="col">Attendee</th><th scope="col">
            <div className="calendar-free-busy-axis">{hours.map(at => <span key={at} style={{ left: `${100 * (at - from) / (to - from)}%` }}>
              {new Intl.DateTimeFormat('en-GB', { timeZone: draft.timezone, hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).format(at)}
            </span>)}</div>
          </th></tr></thead>
          <tbody>{availability.data?.map(item => {
            const conflict = chosen && item.busy.some(b => Date.parse(b.start) < end && Date.parse(b.end) > start)
            return <tr key={item.address}>
              <th scope="row">{item.address}{item.address === person.data?.email?.toLowerCase() && ' (You)'}{conflict && <strong className="calendar-conflict">Conflict</strong>}</th>
              <td><div className="calendar-free-busy-track">
                {item.busy.map(b => <span key={b.start} className="calendar-free-busy-interval" aria-label={`Busy from ${new Date(b.start).toLocaleTimeString([], { timeZone: draft.timezone })} to ${new Date(b.end).toLocaleTimeString([], { timeZone: draft.timezone })}`} style={position(Date.parse(b.start), Date.parse(b.end))} />)}
                {chosen && start < to && end > from && <span className="calendar-free-busy-chosen" aria-label="Chosen time" style={position(start, end)} />}
              </div></td>
            </tr>
          })}</tbody>
        </table>
      </div>}
  </section>
}
