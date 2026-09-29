// `/runs`: the work record, grouped by day.
//
// The list reads every run once and narrows it here, because a chip
// carries the count it would give, and that count has to hold the other
// two chips. One request answers the rows and the nine counts.

import { Menu as MenuIcon } from 'lucide-react'
import { useMemo, useState } from 'react'

import type { ApiClient, RunDto } from '../../api/client'
import { Avatar, Badge, Button, IconButton } from '../../primitives'
import { useAgents, useChannels, useRuns } from '../../queries'
import {
  NO_FILTERS,
  RUN_STATES,
  chipCount,
  failureText,
  groupByDay,
  matchesFilters,
  runDuration,
  runStateBadge,
  triggerText,
  type RunFilters,
} from './runs'

import './runs.css'

interface ChipOption {
  value: string | null
  label: string
}

function FilterChips({
  legend,
  options,
  dimension,
  filters,
  runs,
  onChange,
}: {
  legend: string
  options: ChipOption[]
  dimension: keyof RunFilters
  filters: RunFilters
  runs: readonly RunDto[]
  onChange: (value: string | null) => void
}) {
  return (
    <div className="runs-chip-group" role="group" aria-label={legend}>
      <span className="runs-chip-legend">{legend}</span>
      {options.map((option) => {
        const count = chipCount(runs, filters, dimension, option.value)
        const active = filters[dimension] === option.value
        return (
          <Button
            key={option.value ?? 'all'}
            size="sm"
            shape="pill"
            className="runs-chip"
            aria-pressed={active}
            onClick={() => onChange(active && option.value !== null ? null : option.value)}
          >
            <span>{option.label}</span>
            <span className="runs-chip-count">{count}</span>
          </Button>
        )
      })}
    </div>
  )
}

function RunRow({
  run,
  agentName,
  avatarAppearance,
  channelName,
  onOpen,
}: {
  run: RunDto
  avatarAppearance?: import('../../avatars/catalog').SpriteAppearance
  agentName: string
  channelName: string | null
  onOpen: () => void
}) {
  const badge = runStateBadge(run.state)
  const failure = failureText(run)
  return (
    <Button size="lg" className="runs-row" onClick={onOpen}>
      <Avatar id={run.agent_id} name={agentName} appearance={avatarAppearance} size="sm" />
      <span className="runs-row-what">
        <strong>{agentName}</strong>
        <span className="runs-row-trigger">{triggerText(run, channelName)}</span>
        {failure !== null && (
          <span className="runs-row-failure" title={run.error ?? undefined}>
            {failure}
          </span>
        )}
      </span>
      <Badge tone={badge.tone}>{badge.label}</Badge>
      <span className="runs-row-duration">{runDuration(run)}</span>
    </Button>
  )
}

export function RunsList({
  api,
  onOpenRun,
  onOpenNav,
}: {
  api: ApiClient
  onOpenRun: (runId: string) => void
  onOpenNav: () => void
}) {
  const [filters, setFilters] = useState<RunFilters>(NO_FILTERS)
  const agents = useAgents(api)
  const channels = useChannels(api)
  // The filters narrow the record in the browser, so the query holds no
  // filter of its own.
  const runs = useRuns(api, '', '', '')
  const all = useMemo(() => runs.data ?? [], [runs.data])

  const agentNames = useMemo(
    () => new Map((agents.data ?? []).map((agent) => [agent.id, agent.name])),
    [agents.data],
  )
  const channelNames = useMemo(
    () =>
      new Map(
        (channels.data ?? []).map((channel) => [channel.id, channel.title ?? 'Untitled']),
      ),
    [channels.data],
  )

  const shown = useMemo(
    () => all.filter((run) => matchesFilters(run, filters)),
    [all, filters],
  )
  const days = useMemo(() => groupByDay(shown), [shown])

  const set = (dimension: keyof RunFilters) => (value: string | null) =>
    setFilters((current) => ({ ...current, [dimension]: value }))

  return (
    <div className="runs-panel">
      <header className="runs-header">
        <IconButton
          icon={MenuIcon}
          label="Open conversations"
          variant="ghost"
          className="mobile-navigation-trigger"
          onClick={onOpenNav}
        />
        <h2>Runs</h2>
      </header>
      <p className="runs-summary">Every piece of work a sprite ran for you.</p>

      <div className="runs-filters">
        <FilterChips
          legend="Sprite"
          dimension="agentId"
          filters={filters}
          runs={all}
          onChange={set('agentId')}
          options={[
            { value: null, label: 'All sprites' },
            ...(agents.data ?? []).map((agent) => ({
              value: agent.id,
              label: agent.name,
            })),
          ]}
        />
        <FilterChips
          legend="Channel"
          dimension="channelId"
          filters={filters}
          runs={all}
          onChange={set('channelId')}
          options={[
            { value: null, label: 'All channels' },
            ...(channels.data ?? []).map((channel) => ({
              value: channel.id,
              label: channel.title ?? 'Untitled',
            })),
          ]}
        />
        <FilterChips
          legend="State"
          dimension="state"
          filters={filters}
          runs={all}
          onChange={set('state')}
          options={[
            { value: null, label: 'All states' },
            ...RUN_STATES.map((state) => ({
              value: state,
              label: runStateBadge(state).label,
            })),
          ]}
        />
      </div>

      {days.length === 0 && <p className="runs-empty">No run matches these filters.</p>}
      {days.map((day) => (
        <section className="runs-day" key={day.key} aria-label={day.label}>
          <h3>{day.label}</h3>
          {day.runs.map((run) => (
            <RunRow
              avatarAppearance={(agents.data ?? []).find((agent) => agent.id === run.agent_id)?.avatar}
              key={run.id}
              run={run}
              agentName={agentNames.get(run.agent_id) ?? 'A sprite'}
              channelName={
                run.channel_id == null ? null : channelNames.get(run.channel_id) ?? null
              }
              onOpen={() => onOpenRun(run.id)}
            />
          ))}
        </section>
      ))}
    </div>
  )
}
