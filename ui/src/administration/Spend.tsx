// Cost and token spend per person, for a period.
//
// This is the read that answers the first billing question: who spent
// the budget. The biggest spender is first, and a person under a cap
// that they reached says so.

import { useState } from 'react'

import type { ApiClient, PersonUsageDto } from '../api/client'
import { Badge, Frame, Row, SectionLabel, Select } from '../primitives'
import { useInstallationUsage } from '../queries'
import { money, personLabel } from '../components/settings/People'

/** The periods the page offers. The month is the one the Spend Cap
 *  counts, so it is the default. */
const PERIODS = [
  { value: 'month', label: 'This calendar month' },
  { value: '7', label: 'The last 7 days' },
  { value: '30', label: 'The last 30 days' },
]

const DAY_MS = 24 * 60 * 60 * 1000

function periodOf(value: string): { from?: number; to?: number } {
  if (value === 'month') return {}
  const days = Number(value)
  const now = Date.now()
  return { from: now - days * DAY_MS, to: now }
}

function tokens(count: number): string {
  return count.toLocaleString()
}

function PersonRow({ item }: { item: PersonUsageDto }) {
  return (
    <Row className="spend-row">
      <span className="spend-person">{personLabel(item.person)}</span>
      <span className="spend-tokens">
        {tokens(item.total.input_tokens)} in · {tokens(item.total.output_tokens)} out ·{' '}
        {item.total.calls} {item.total.calls === 1 ? 'call' : 'calls'}
      </span>
      {item.cap_reached && <Badge tone="failed">Cap reached</Badge>}
      <span className="spend-cost">{money(item.total.cost_usd)}</span>
    </Row>
  )
}

export function Spend({ api }: { api: ApiClient }) {
  const [period, setPeriod] = useState('month')
  const usage = useInstallationUsage(api, periodOf(period))
  const items = usage.data?.items ?? []

  return (
    <section className="administration-section">
      <div className="administration-title">
        <h2>Spend</h2>
        <span>What each person's sprites spent on model calls.</span>
      </div>
      <Frame hint="The cost is what each serving model's own price list makes of the tokens it reported. Pagis stores what a call cost and sends nobody an invoice.">
        <Row>
          <span className="spend-total">{money(usage.data?.total.cost_usd ?? 0)}</span>
          <span className="spend-total-note">
            {tokens(usage.data?.total.input_tokens ?? 0)} tokens in ·{' '}
            {tokens(usage.data?.total.output_tokens ?? 0)} tokens out ·{' '}
            {usage.data?.total.calls ?? 0} model calls
          </span>
          <span className="administration-row-trailing">
            <Select
              label="Period"
              value={period}
              onValueChange={setPeriod}
              items={PERIODS}
            />
          </span>
        </Row>
      </Frame>

      <SectionLabel>Per person</SectionLabel>
      <Frame>
        {items.length === 0 ? (
          <Row>
            <span className="administration-note">
              {usage.isError
                ? 'The spend could not be read.'
                : 'Nobody spent anything in this period.'}
            </span>
          </Row>
        ) : (
          items.map((item) => <PersonRow key={item.person.id} item={item} />)
        )}
      </Frame>
    </section>
  )
}
