// The parts of a Subject Page (ADR-0007): the compiled truth, the
// Facts table and the open Schedules above the rule, the Timeline
// below it. Reflection rewrites the part above the rule only.

import { Badge } from '../../primitives'
import { changeTimeLabel } from './pages'
import { parseSourceReference, type SubjectPage } from './subjectPage'

export function SubjectPageBody({
  page,
  writer,
  connectionName,
}: {
  page: SubjectPage
  /** The name of the Agent that rewrites the page. */
  writer: string
  connectionName: (connectionId: string) => string
}) {
  return (
    <>
      {page.truth !== '' && <p className="memory-truth">{page.truth}</p>}

      {page.facts.length > 0 && (
        <table className="memory-facts" aria-label="Facts">
          <tbody>
            {page.facts.map((fact, index) => (
              <tr key={index} data-status={fact.status}>
                <td className="memory-facts-kind">{fact.kind}</td>
                <td>
                  {fact.status === 'active' ? fact.claim : <s>{fact.claim}</s>}
                  {fact.note !== null && (
                    <span className="memory-facts-note"> · {fact.note}</span>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {page.schedules.map((schedule) => (
        <div key={schedule.id} className="memory-schedule">
          <Badge tone="waiting">Schedule</Badge>
          <span>{schedule.purpose}</span>
          <span className="memory-schedule-due">
            Due {new Date(schedule.dueAt).toLocaleString(undefined, {
              month: 'short',
              day: 'numeric',
              hour: '2-digit',
              minute: '2-digit',
            })}
          </span>
        </div>
      ))}

      <hr className="memory-rule" />

      <ol className="memory-timeline" aria-label="Timeline">
        {page.timeline.map((entry) => {
          const source = parseSourceReference(entry.sourceReference)
          return (
            <li key={entry.sourceReference} className="memory-timeline-entry">
              <span className="memory-timeline-time">{changeTimeLabel(entry.sourceTime)}</span>
              <span className="memory-timeline-body">
                <span className="memory-timeline-words">{entry.words}</span>
                <span className="memory-timeline-meta">
                  {[source.resource, source.connectionId && connectionName(source.connectionId)]
                    .filter(Boolean)
                    .join(' · ')}
                </span>
              </span>
            </li>
          )
        })}
      </ol>
      <p className="memory-note">
        Above the rule {writer} rewrites; below it the source words stay as they arrived.
        Reflection cannot change a Timeline entry.
      </p>
    </>
  )
}
