// The sprite roster: every sprite you hired, with a face, what it
// is doing now, the lines it answers on, and when it last worked. A row
// opens the profile at `/sprites/:agentId`.
//
// "Hire a sprite" is a URL of its own (`/sprites?new=1`), so the command
// palette and a link can both open the hiring flow.

import type { AgentDto, ApiClient } from '../../api/client'
import { ChevronRight, Plus, Users } from 'lucide-react'
import { Avatar, Badge, Button } from '../../primitives'
import { useAgents, usePhoneNumbers, useRuns, useWorkspace } from '../../queries'
import { agentPresence, usePresence } from '../../state/presence'
import { formatE164 } from '../AgentPhoneNumber'
import { HireAgent } from './HireAgent'
import { PageState } from '../PageState'
import { activityTone, activityWord } from '../stateWords'
import { lastActiveAt, lastActiveLabel } from './labels'

import '../agent.css'
import '../settings.css'
import './sprites.css'

function RosterRow({
  agent,
  phone,
  lastActive,
  chief,
  onOpen,
}: {
  agent: AgentDto
  phone: string | null
  lastActive: number | null
  /** The Workspace names this Agent its Chief of Staff (ADR-0022). */
  chief: boolean
  onOpen: () => void
}) {
  const presence = usePresence((state) => agentPresence(state, agent.id))
  return (
    <Button
      size="xl"
      className="sprite-row"
      data-testid="sprite-row"
      aria-label={`${agent.name}, ${activityWord(presence).toLowerCase()}`}
      onClick={onOpen}
    >
      <Avatar id={agent.id} name={agent.name} appearance={agent.avatar} size="lg" presence={presence} />
      <span className="sprite-row-body">
        <span className="sprite-row-head">
          <span className="agent-name">{agent.name}</span>
          <span className="agent-job">{agent.job}</span>
        </span>
        <span className="sprite-row-chips">
          <Badge tone={activityTone(presence)}>{activityWord(presence)}</Badge>
          {chief && <Badge tone="neutral">Chief of Staff</Badge>}
          {phone !== null && <Badge tone="neutral">{formatE164(phone)}</Badge>}
          {agent.email_address != null && (
            <Badge tone="neutral">{agent.email_address}</Badge>
          )}
        </span>
        <span className="sprite-row-last">{lastActiveLabel(lastActive)}</span>
      </span>
      <ChevronRight className="sprite-row-open" size={16} aria-hidden />
    </Button>
  )
}

export function SpriteRoster({
  api,
  creating,
  onCreating,
  onOpenAgent,
  onOpenChannel,
}: {
  api: ApiClient
  /** `/sprites?new=1` opens the hiring form. */
  creating: boolean
  onCreating: (creating: boolean) => void
  onOpenAgent: (agentId: string) => void
  /** Hiring ends in the new agent's DM. */
  onOpenChannel: (channelId: string) => void
}) {
  const agents = useAgents(api)
  const numbers = usePhoneNumbers(api)
  const runs = useRuns(api, '', '', '')
  const workspace = useWorkspace(api)
  const roster = (agents.data ?? []).filter((agent) => agent.status !== 'archived')

  return (
    <section className="sprite-roster" data-testid="sprite-roster">
      <header className="sprite-header">
        <div>
          <h2>Sprites</h2>
          <p className="page-intro">Meet the sprites on your team.</p>
        </div>
        {!creating && (
          <Button variant="primary" onClick={() => onCreating(true)}><Plus size={16} aria-hidden />Hire a sprite</Button>
        )}
      </header>
      <p className="sprite-description">Message a sprite directly, or use @mentions to bring it into a group conversation.</p>
      <HireAgent
        api={api}
        open={creating}
        onCancel={() => onCreating(false)}
        onHired={(channelId) => {
          onCreating(false)
          onOpenChannel(channelId)
        }}
      />
      {agents.isPending && <PageState icon={Users} title="Loading sprites…" />}
      {agents.isError && <PageState icon={Users} title="Could not load your sprites" onRetry={() => { void agents.refetch() }} />}
      {agents.isSuccess && roster.length === 0 && !creating && (
        <PageState icon={Users} title="Build your team">
          No sprite yet. Hire your first one with Hire a sprite.
        </PageState>
      )}
      <div className="sprite-list">
        {roster.map((agent) => (
          <RosterRow
            key={agent.id}
            agent={agent}
            phone={
              (numbers.data?.items ?? []).find(
                (number) => number.agent_id === agent.id,
              )?.e164 ?? null
            }
            lastActive={lastActiveAt(runs.data ?? [], agent.id)}
            chief={workspace.data?.chief_of_staff_agent_id === agent.id}
            onOpen={() => onOpenAgent(agent.id)}
          />
        ))}
      </div>
    </section>
  )
}
