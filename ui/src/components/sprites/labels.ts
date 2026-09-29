// The words the roster and the profile say about one Agent.

import type { RunDto } from '../../api/client'

/** The newest moment a run of this Agent moved, or `null` where the
 *  Agent has no run yet. A live run counts as now. */
export function lastActiveAt(runs: readonly RunDto[], agentId: string): number | null {
  let newest: number | null = null
  for (const run of runs) {
    if (run.agent_id !== agentId) continue
    const at = run.ended_at ?? run.started_at ?? run.created_at
    if (newest === null || at > newest) newest = at
  }
  return newest
}

export function lastActiveLabel(at: number | null): string {
  return at === null ? 'No work yet' : `Last active ${new Date(at).toLocaleString()}`
}
