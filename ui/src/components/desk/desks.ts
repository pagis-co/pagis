// Which Desks the Desk Panel shows (ADR-0022). The module knows
// nothing about React, so the rule is tested on its own.
//
// Home is the office, so it lists every Desk with the Chief of Staff's
// first. The Chief of Staff's direct channel is one conversation, so it
// shows that one Desk, and the Desk of an Agent joins it only while a
// Delegation the channel waits on is open.

import type { AgentDto } from '../../api/client'
import type { LiveRun } from '../../state/presence'

/** The surface the panel sits on. Each one owns a different rule. */
export type DeskSurface = 'home' | 'channel'

export interface DeskRows {
  /** The Desk that opens expanded; `null` when no Agent is active. */
  chief: AgentDto | null
  /** The Desks under it, compact. */
  others: AgentDto[]
}

export function deskRows({
  agents,
  chiefId,
  surface,
  channelId,
  runs,
}: {
  agents: readonly AgentDto[]
  chiefId: string | null | undefined
  surface: DeskSurface
  /** The open channel; `null` on Home. */
  channelId: string | null
  /** The unfinished Runs the presence store holds. */
  runs: readonly LiveRun[]
}): DeskRows {
  const active = agents.filter((agent) => agent.status === 'active')
  const chief = active.find((agent) => agent.id === chiefId) ?? null
  if (surface === 'home') {
    return { chief, others: active.filter((agent) => agent.id !== chief?.id) }
  }
  const delegates = new Set(
    runs
      .filter((run) => run.originChannelId === channelId && run.agentId !== chief?.id)
      .map((run) => run.agentId),
  )
  return { chief, others: active.filter((agent) => delegates.has(agent.id)) }
}
