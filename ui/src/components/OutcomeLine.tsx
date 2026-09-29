// A Run that ended with nothing to say. The reader sees a quiet
// line under the Agent's name — a badge, one sentence, and the act
// that comes next — and never a badge on an empty message.
//
// The sentence comes from the Run steps: the stop says who
// stopped it and after how long, the failure says why.

import { Link } from '@tanstack/react-router'

import type { ApiClient } from '../api/client'
import { Avatar, Badge, Button } from '../primitives'
import { useRunSteps } from '../queries'
import { formatClock, formatDuration, type RunOutcome, type TimelineRow } from '../timeline'

import './OutcomeLine.css'

export function OutcomeLine({
  api,
  row,
  outcome,
  authorName,
  authorAppearance,
  onAskAgain,
}: {
  api: ApiClient
  row: TimelineRow
  outcome: RunOutcome
  authorName: string
  authorAppearance?: import('../avatars/catalog').SpriteAppearance
  /** Puts the question back in the composer, ready to send again. */
  onAskAgain: () => void
}) {
  const steps = useRunSteps(api, row.runId)
  const stopped = steps.data?.stopped
  const sentence =
    outcome === 'Stopped'
      ? stopped == null
        ? 'The Run stopped before it answered.'
        : `${stopped.by === 'user' ? 'You' : stopped.by} stopped it after ${formatDuration(stopped.after_ms)}, before it answered.`
      : (steps.data?.failure ?? 'The Run failed.')
  return (
    <div className="outcome" data-testid="outcome-line">
      <Avatar
        id={row.authorAgentId ?? 'agent'}
        name={authorName}
        appearance={authorAppearance}
        size="md"
      />
      <div className="outcome-body">
        <div className="outcome-name">
          <span className="outcome-author">{authorName}</span>
          <span className="outcome-time">
            {formatClock(row.createdAt)}
          </span>
        </div>
        <div className="outcome-quiet">
          <Badge tone={outcome === 'Stopped' ? 'neutral' : 'failed'}>
            {outcome}
          </Badge>
          <span>{sentence}</span>
          {outcome === 'Stopped' ? (
            <Button size="sm" variant="ghost" onClick={onAskAgain}>
              Ask again
            </Button>
          ) : (
            <>
              <Button size="sm" onClick={onAskAgain}>
                Retry
              </Button>
              {row.runId != null && (
                <Link
                  to="/runs/$runId"
                  params={{ runId: row.runId }}
                  className="outcome-run"
                >
                  Open the Run
                </Link>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  )
}
