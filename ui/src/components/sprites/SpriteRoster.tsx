// The sprite roster: every sprite you made, with a face, what it
// is doing now, the lines it answers on, and when it last worked. A row
// opens the profile at `/sprites/:agentId`.
//
// "New sprite" is a URL of its own (`/sprites?new=1`), so the command
// palette and a link can both open the creating flow.

import type { AgentDto, ApiClient } from '../../api/client'
import { ChevronRight, Plus, Users } from 'lucide-react'
import { Avatar, Badge, Button, Frame, IconButton, Row } from '../../primitives'
import { useAgents, usePhoneNumbers, useRuns, useWorkspace } from '../../queries'
import { agentPresence, usePresence } from '../../state/presence'
import { formatE164 } from '../AgentPhoneNumber'
import { NewSprite } from './NewSprite'
import { PageState } from '../PageState'
import { activityTone, activityWord } from '../stateWords'
import { lastActiveAt, lastActiveLabel } from './labels'

import '../agent.css'
import '../settings.css'
import './sprites.css'
import { useIsMobile } from '../../state/useIsMobile'
import { changeTimeLabel } from '../memory/pages'
import { LargeTitle } from '../phone/TopBar'

function RosterRow({
  agent,
  number,
  lastActive,
  chief,
  onOpen,
  phone,
}: {
  phone: boolean
  agent: AgentDto
  number: string | null
  lastActive: number | null
  /** The Workspace names this Agent its Chief of Staff (ADR-0022). */
  chief: boolean
  onOpen: () => void
}) {
  const presence = usePresence((state) => agentPresence(state, agent.id))
  if (phone) {
    const minutes = lastActive === null ? null : Math.max(0, Math.floor((Date.now() - lastActive) / 60000))
    const last = lastActive === null ? 'No work yet' : minutes !== null && minutes < 60 ? `${minutes} min ago` : changeTimeLabel(lastActive)
    return <Frame><Row roomy className="phone-sprite-card" data-testid="sprite-row" aria-label={`${agent.name}, ${activityWord(presence).toLowerCase()}`} onClick={onOpen}>
      <Avatar id={agent.id} name={agent.name} appearance={agent.avatar} size="xl" presence={presence} />
      <span className="phone-row-copy"><strong>{agent.name}</strong><span className="phone-hint">{agent.job}{agent.description ? `. ${agent.description}` : ''}</span><span className="sprite-row-chips"><Badge tone={activityTone(presence)}>{activityWord(presence)}</Badge>{chief && <Badge tone="accent">Main sprite</Badge>}{number !== null && <Badge>{formatE164(number)}</Badge>}{agent.email_address && <Badge>{agent.email_address}</Badge>}{presence !== 'working' && <span className="phone-row-time">Last active {last}</span>}</span></span>
    </Row></Frame>
  }
  return (
    <Button
      size="xl"
      className="sprite-row"
      data-testid="sprite-row"
      aria-label={`${agent.name}, ${activityWord(presence).toLowerCase()}`}
      onClick={onOpen}
    >
      <Avatar id={agent.id} name={agent.name} appearance={agent.avatar} size={phone ? "xl" : "lg"} presence={presence} />
      <span className="sprite-row-body">
        <span className="sprite-row-head">
          <span className="agent-name">{agent.name}</span>
          <span className="agent-job">{agent.job}</span>
        </span>
        {phone && <span className="phone-hint">{agent.description}</span>}
        <span className="sprite-row-chips">
          <Badge tone={activityTone(presence)}>{activityWord(presence)}</Badge>
          {chief && <Badge tone="neutral">Main sprite</Badge>}
          {number !== null && <Badge tone="neutral">{formatE164(number)}</Badge>}
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
  /** `/sprites?new=1` opens the creating form. */
  creating: boolean
  onCreating: (creating: boolean) => void
  onOpenAgent: (agentId: string) => void
  /** Creation opens the new agent's DM. */
  onOpenChannel: (channelId: string) => void
}) {
  const phone = useIsMobile()
  const agents = useAgents(api)
  const numbers = usePhoneNumbers(api)
  const runs = useRuns(api, '', '', '')
  const workspace = useWorkspace(api)
  const roster = (agents.data ?? []).filter((agent) => agent.status !== 'archived')

  return (
    <section className={`sprite-roster${phone ? " sprite-roster-phone" : ""}`} data-testid="sprite-roster">
      {phone ? <LargeTitle title="Sprites" hint="Meet the sprites on your team." actions={<IconButton icon={Plus} label="New sprite" variant="link" onClick={() => onCreating(true)} />} /> : <header className="sprite-header">
        <div>
          <h2>Sprites</h2>
          <p className="page-intro">Meet the sprites on your team.</p>
        </div>
        {!creating && (
          <Button variant="primary" onClick={() => onCreating(true)}><Plus size={16} aria-hidden />New sprite</Button>
        )}
      </header>}
      {!phone && <p className="sprite-description">Message a sprite directly, or use @mentions to bring it into a group conversation.</p>}
      <NewSprite
        api={api}
        open={creating}
        onCancel={() => onCreating(false)}
        onCreated={(channelId) => {
          onCreating(false)
          onOpenChannel(channelId)
        }}
      />
      {agents.isPending && <PageState icon={Users} title="Loading sprites…" />}
      {agents.isError && <PageState icon={Users} title="Could not load your sprites" onRetry={() => { void agents.refetch() }} />}
      {agents.isSuccess && roster.length === 0 && !creating && (
        <PageState icon={Users} title="Build your team">
          No sprite yet. Make your first one with New sprite.
        </PageState>
      )}
      <div className="sprite-list">
        {roster.map((agent) => (
          <RosterRow
            key={agent.id}
            agent={agent}
            phone={phone}
            number={
              (numbers.data?.items ?? []).find(
                (number) => number.agent_id === agent.id,
              )?.e164 ?? null
            }
            lastActive={lastActiveAt(runs.data ?? [], agent.id)}
            chief={workspace.data?.chief_of_staff_agent_id === agent.id}
            onOpen={() => onOpenAgent(agent.id)}
          />
        ))}
        {phone && <Button variant="dashed" size="xl" onClick={() => onCreating(true)}><span className="phone-new-sprite-face"><Plus size={20} aria-hidden /></span><span className="phone-row-copy"><strong>New sprite</strong><span className="phone-hint">Make a sprite for one kind of work, such as travel, your inbox or bookkeeping.</span></span></Button>}
      </div>
    </section>
  )
}
