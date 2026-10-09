// The Desk Panel (ADR-0022): the inspector's own tenant on Home
// and in the Chief of Staff's direct channel.
//
// The Chief of Staff's Desk gets the whole width, with the step list of
// its live Run under it. The other Desks sit below it compact, so the
// whole office is one glance. The full-page screen view stays the
// user's own: the Desk of the panel opens it from its Expand button and
// never on its own. The panel is derived from the route, so it has
// no toggle of its own; Call, Mail and Thread take the slot while they
// are open and the panel returns when they close.

import { MonitorPlay, UserPlus } from 'lucide-react'

import type { AgentDto, ApiClient } from '../../api/client'
import { Avatar, Badge, Button } from '../../primitives'
import {
  useAgents,
  useAwakeCount,
  useComputer,
  useComputerDisk,
  useOnboarding,
  useSleepComputer,
  useWakeComputer,
  useWorkspace,
} from '../../queries'
import { agentPresence, usePresence } from '../../state/presence'
import { ComputerTile, NoDocker } from '../Computers'
import { chiefOfStaff } from '../sidebar/conversations'
import { activityTone, activityWord, computerStateWord } from '../stateWords'
import { DeskSteps } from './DeskSteps'
import { deskRows, type DeskSurface } from './desks'
import { diskFigure } from './disk'

import './desk.css'

/** One Desk under the first: a face, who it is, what it is doing, and
 *  the one control a compact row needs. Only the Chief of Staff's Desk
 *  shows the screen, so a row carries no live view. */
function CompactDesk({
  api,
  agent,
  onOpenAgent,
}: {
  api: ApiClient
  agent: AgentDto
  onOpenAgent: (agentId: string) => void
}) {
  const computer = useComputer(api, agent.id)
  const wake = useWakeComputer(api, agent.id)
  const sleep = useSleepComputer(api, agent.id)
  const presence = usePresence((state) => agentPresence(state, agent.id))
  const caption = usePresence((state) => {
    for (const run of Object.values(state.runs)) {
      if (run.agentId === agent.id) return run.caption
    }
    return null
  })
  const state = computer.data?.state ?? 'off'
  return (
    <div className="desk-compact" data-testid="desk-compact">
      <Button
        size="lg"
        className="desk-compact-who"
        aria-label={`Open ${agent.name}`}
        onClick={() => onOpenAgent(agent.id)}
      >
        <Avatar
          id={agent.id}
          name={agent.name}
          appearance={agent.avatar}
          size="sm"
          presence={presence}
        />
        <span className="desk-compact-name">{agent.name}</span>
        <span className="desk-compact-caption">
          {caption ?? computerStateWord(state, computer.data?.percent)}
        </span>
      </Button>
      {state === 'off' || state === 'failed' ? (
        <Button
          size="sm"
          variant="link"
          aria-label={`Wake ${agent.name}\u2019s computer`}
          disabled={wake.isPending}
          onClick={() => wake.mutate()}
        >
          Wake
        </Button>
      ) : (
        <Button
          size="sm"
          variant="link"
          aria-label={`Put ${agent.name}\u2019s computer to sleep`}
          disabled={sleep.isPending}
          onClick={() => sleep.mutate()}
        >
          Put to sleep
        </Button>
      )}
    </div>
  )
}

/** The footer: how many desks, how many are awake,
 *  and what the office keeps on disk. A disk Docker could not measure
 *  is left out rather than guessed at. */
function DeskFooter({
  api,
  desks,
  disk,
}: {
  api: ApiClient
  desks: number
  disk: string | null
}) {
  const agents = useAgents(api)
  const awake = useAwakeCount(
    api,
    (agents.data ?? []).filter((agent) => agent.status === 'active').map((agent) => agent.id),
  )
  return (
    <p className="desk-panel-footer" data-testid="desk-footer">
      <MonitorPlay size={14} aria-hidden />
      <span>{desks === 1 ? '1 desk' : `${desks} desks`}</span>
      <span>·</span>
      <span>{awake} awake</span>
      {disk !== null && (
        <>
          <span>·</span>
          <span>{disk}</span>
        </>
      )}
    </p>
  )
}

export function DeskPanel({
  api,
  surface,
  channelId,
  onOpenAgent,
  onOpenChannel,
  onNew,
}: {
  api: ApiClient
  surface: DeskSurface
  /** The open channel; `null` on Home. */
  channelId: string | null
  onOpenAgent: (agentId: string) => void
  /** Opens the Chief of Staff's direct channel from Home. */
  onOpenChannel: (agentId: string) => void
  onNew: () => void
}) {
  const agents = useAgents(api)
  const workspace = useWorkspace(api)
  const onboarding = useOnboarding(api)
  const disk = useComputerDisk(api)
  const runs = usePresence((state) => state.runs)
  const chief = chiefOfStaff(agents.data ?? [], workspace.data?.chief_of_staff_agent_id)
  const chiefPresence = usePresence((state) =>
    chief === null ? 'none' : agentPresence(state, chief.id),
  )
  const liveRun =
    chief === null
      ? null
      : (Object.values(runs).find((run) => run.agentId === chief.id) ?? null)
  const rows = deskRows({
    agents: agents.data ?? [],
    chiefId: workspace.data?.chief_of_staff_agent_id,
    surface,
    channelId,
    runs: Object.values(runs),
  })

  return (
    <div className="desk-panel" data-testid="desk-panel">
      <header className="desk-panel-header">
        <h2>{chief === null ? 'Desks' : `${chief.name}\u2019s desk`}</h2>
        {chief !== null && (
          <Badge tone={activityTone(chiefPresence)}>{activityWord(chiefPresence)}</Badge>
        )}
        {chief !== null && surface === 'home' && (
          <Button
            size="sm"
            variant="link"
            className="desk-panel-open-thread"
            onClick={() => onOpenChannel(chief.id)}
          >
            Open thread
          </Button>
        )}
      </header>

      {onboarding.data !== undefined && onboarding.data.docker.endpoint == null && <NoDocker />}

      {rows.chief === null ? (
        <p className="desk-panel-empty">
          No sprite is active, so no desk is in use.
        </p>
      ) : (
        <>
          <ComputerTile api={api} agent={rows.chief} />
          <DeskSteps api={api} runId={liveRun?.runId ?? null} />
        </>
      )}

      {rows.others.length > 0 && (
        <div className="desk-others">
          <p className="desk-section-heading">Other desks</p>
          {rows.others.map((agent) => (
            <CompactDesk
              key={agent.id}
              api={api}
              agent={agent}
              onOpenAgent={onOpenAgent}
            />
          ))}
        </div>
      )}

      {surface === 'home' && (
        <Button
          size="lg"
          className="desk-new"
          aria-label="New sprite"
          onClick={onNew}
        >
          <UserPlus size={16} aria-hidden />
          An empty desk · New sprite
        </Button>
      )}

      <DeskFooter
        api={api}
        desks={rows.others.length + (rows.chief === null ? 0 : 1)}
        disk={diskFigure(disk.data?.bytes)}
      />
    </div>
  )
}
