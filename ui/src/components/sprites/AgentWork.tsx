// The Work section of an Agent profile: the runs this Agent
// has, newest first. A row opens the run record at `/runs/:runId`.

import type { ApiClient } from '../../api/client'
import { Button } from '../../primitives'
import { useRuns } from '../../queries'

import './sprites.css'

function duration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return 'In progress'
  if (ms < 1_000) return `${ms} ms`
  return `${(ms / 1_000).toFixed(1)} s`
}

export function AgentWork({
  api,
  agentId,
  agentName,
  onOpenRun,
}: {
  api: ApiClient
  agentId: string
  agentName: string
  onOpenRun: (runId: string) => void
}) {
  const runs = useRuns(api, agentId, '', '')
  const items = runs.data ?? []

  if (items.length === 0) {
    return <p className="settings-hint">{agentName} has no run yet.</p>
  }
  return (
    <div className="agent-work" data-testid="agent-work">
      {items.map((run) => (
        <Button
          key={run.id}
          size="lg"
          className="agent-work-row"
          aria-label={`${run.state.replaceAll('_', ' ')} ${duration(run.duration_ms)}`}
          onClick={() => onOpenRun(run.id)}
        >
          <span className={`run-state run-state-${run.state}`}>
            {run.state.replaceAll('_', ' ')}
          </span>
          <span>{duration(run.duration_ms)}</span>
          <span>{new Date(run.created_at).toLocaleString()}</span>
        </Button>
      ))}
    </div>
  )
}
