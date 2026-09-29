// The Usage section: what the signed-in person spent on model
// calls this month, the cap they are under, and the same figure per run.
//
// Every person reads their own, whatever their role: a member who cannot
// see the installation's keys can still see what their own sprites
// spent. On a local installation the one person has no cap, so the page
// reads as an accounting of their own work and nothing else.

import type { ApiClient, RunSpendDto } from '../../api/client'
import { Badge, Frame, Row, SectionLabel } from '../../primitives'
import { useMyUsage } from '../../queries'

import './Usage.css'

/** US dollars, to the cent. */
function money(amount: number): string {
  return `$${amount.toFixed(2)}`
}

/** A token count with thousands separators. */
function tokens(count: number): string {
  return count.toLocaleString()
}

function RunRow({ run }: { run: RunSpendDto }) {
  return (
    <Row className="usage-run">
      <span className="usage-run-when">
        {new Date(run.last_at).toLocaleString()}
      </span>
      <span className="usage-run-tokens">
        {tokens(run.total.input_tokens)} in · {tokens(run.total.output_tokens)} out
      </span>
      <span className="usage-run-calls">
        {run.total.calls} {run.total.calls === 1 ? 'call' : 'calls'}
      </span>
      <span className="usage-run-cost">{money(run.total.cost_usd)}</span>
    </Row>
  )
}

export function Usage({ api }: { api: ApiClient }) {
  const usage = useMyUsage(api)
  const total = usage.data?.total
  const cap = usage.data?.monthly_spend_cap_usd ?? null
  const atCap = cap !== null && (total?.cost_usd ?? 0) >= cap

  return (
    <section className="settings-section usage">
      <div className="usage-title">
        <h1>Usage</h1>
        <span>What your sprites spent on model calls this month.</span>
      </div>
      <Frame hint="The cost is what the serving model's own price list makes of the tokens it reported. An administrator of this installation sets the cap and can raise it.">
        <Row>
          <span className="usage-total">{money(total?.cost_usd ?? 0)}</span>
          {cap === null ? (
            <span className="usage-cap">No monthly cap</span>
          ) : (
            <span className="usage-cap">of a {money(cap)} monthly cap</span>
          )}
          {atCap && <Badge tone="failed">Cap reached</Badge>}
        </Row>
        <Row className="usage-tokens">
          <span>
            {tokens(total?.input_tokens ?? 0)} tokens in ·{' '}
            {tokens(total?.output_tokens ?? 0)} tokens out ·{' '}
            {total?.calls ?? 0} model calls
          </span>
        </Row>
      </Frame>
      <SectionLabel>Per run</SectionLabel>
      <Frame>
        {(usage.data?.runs ?? []).length === 0 ? (
          <span className="usage-empty">No model calls this month.</span>
        ) : (
          (usage.data?.runs ?? []).map((run) => <RunRow key={run.run_id} run={run} />)
        )}
      </Frame>
    </section>
  )
}
