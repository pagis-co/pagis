import type { ApiClient } from '../../api/client'
import { Badge, Button, Frame, Row, SectionLabel } from '../../primitives'
import { useIsMobile } from '../../state/useIsMobile'
import { useRuns } from '../../queries'
import { groupByDay, runDuration, runStateBadge, triggerText } from '../runs/runs'
import './sprites.css'

export function AgentWork({ api, agentId, agentName, onOpenRun }: { api: ApiClient; agentId: string; agentName: string; onOpenRun: (runId: string) => void }) {
  const runs = useRuns(api, agentId, '', '')
  const phone = useIsMobile()
  if (!phone) return runs.data?.length ? <div className="agent-work" data-testid="agent-work">{runs.data.map((run) => <Button key={run.id} size="lg" className="agent-work-row" aria-label={`${run.state.replaceAll('_', ' ')} ${runDuration(run)}`} onClick={() => onOpenRun(run.id)}><span className={`run-state run-state-${run.state}`}>{run.state.replaceAll('_', ' ')}</span><span>{run.title}</span><span>{runDuration(run)}</span><span>{new Date(run.created_at).toLocaleString()}</span></Button>)}</div> : <p className="settings-hint">{agentName} has no run yet.</p>
  return <div className="agent-work" data-testid="agent-work">
    {runs.isPending && <p className="phone-hint">Reading the work…</p>}
    {runs.isError && <p className="phone-hint" role="alert">Could not read the work record.</p>}
    {runs.data?.length === 0 && <p className="phone-hint">{agentName} has no run yet.</p>}
    {groupByDay(runs.data ?? []).map((day) => <section className="phone-section" key={day.key}><SectionLabel>{day.label}</SectionLabel><Frame>{day.runs.map((run) => { const badge = runStateBadge(run.state); return <Row key={run.id} onClick={() => onOpenRun(run.id)}><span className="phone-row-copy"><span>{run.title}</span><span className="phone-hint">{triggerText(run, null)} · {runDuration(run)} · {new Date(run.created_at).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' })}</span></span><Badge tone={badge.tone}>{badge.label}</Badge></Row> })}</Frame></section>)}
  </div>
}
