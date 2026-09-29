// The step list of one live Run, as the Desk Panel reads it
// (ADR-0022). The step that is running is at the top, so
// the list counts down; the work record under a reply counts up,
// because a finished Run reads in the order it happened.

import type { ApiClient } from '../../api/client'
import { useRunSteps } from '../../queries'
import { formatDuration } from '../../timeline'

export function DeskSteps({ api, runId }: { api: ApiClient; runId: string | null }) {
  const steps = useRunSteps(api, runId)
  const rows = [...(steps.data?.steps ?? [])].reverse()
  if (runId === null) return null
  return (
    <div className="desk-steps" data-testid="desk-steps">
      <p className="desk-section-heading">Steps</p>
      {rows.length === 0 ? (
        <p className="desk-steps-none">No tool has run yet.</p>
      ) : (
        <ol className="desk-steps-list">
          {rows.map((step) => (
            <li className="desk-step" key={step.index}>
              <span className="desk-step-index">{step.index}</span>
              <span className="desk-step-label">{step.label}</span>
              <span className="desk-step-duration">
                {step.duration_ms == null ? 'now' : formatDuration(step.duration_ms)}
              </span>
            </li>
          ))}
        </ol>
      )}
    </div>
  )
}
