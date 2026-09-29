// The conversation list view model (ADR-0022). The Chief of
// Staff comes first, the other Agents next, the groups last. The
// module knows nothing about React, so the order is tested on its own.

import type { AgentDto, ChannelDto } from '../../api/client'

export type ConversationKind = 'chief' | 'agent' | 'group' | 'agents'

export interface ConversationRow {
  kind: ConversationKind
  channel: ChannelDto
  /** The one Agent of the user's own DM; `null` for every other row. */
  agent: AgentDto | null
}

/**
 * The Chief of Staff: the Agent the Workspace names (ADR-0022). The
 * caller passes `chiefId` from the Workspace setting; an Agent that is
 * archived or off the roster names nobody, and so does a Workspace that
 * is not read yet.
 */
export function chiefOfStaff(
  agents: readonly AgentDto[],
  chiefId: string | null | undefined,
): AgentDto | null {
  if (chiefId === null || chiefId === undefined) return null
  return (
    agents.find((agent) => agent.id === chiefId && agent.status === 'active') ?? null
  )
}

/**
 * The channels in reading order. A DM whose Agent is not on the roster
 * reads as a group, because a group is the general case. A channel the
 * user is not a member of is an Agent channel: it belongs to the
 * Agents that talk in it, and the user reads it (ADR-0003).
 */
export function conversationRows(
  channels: readonly ChannelDto[],
  agents: readonly AgentDto[],
  chiefId: string | null | undefined,
): ConversationRow[] {
  const chief = chiefOfStaff(agents, chiefId)
  const rows = channels.map((channel): ConversationRow => {
    if (!channel.user_member) return { kind: 'agents', channel, agent: null }
    const agentId = channel.kind === 'dm' ? channel.agent_ids[0] : undefined
    const agent = agents.find((candidate) => candidate.id === agentId) ?? null
    if (agent === null) return { kind: 'group', channel, agent: null }
    return { kind: agent.id === chief?.id ? 'chief' : 'agent', channel, agent }
  })
  const order: readonly ConversationKind[] = ['chief', 'agent', 'group', 'agents']
  return rows.sort((a, b) => order.indexOf(a.kind) - order.indexOf(b.kind))
}
