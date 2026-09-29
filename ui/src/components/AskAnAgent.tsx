// "Ask a sprite to set one up". An empty Automations or
// Software section is a dead end unless it says what to do next, so
// every one of them carries this control: it names the agents, and the
// chosen agent's DM opens with the first message already written. The
// draft is a message, not a command: the user edits it and sends it.

import { Button, Menu } from '../primitives'

/** The kind of work the reader wants an Agent to set up. */
export type AskKind = 'automation' | 'schedule' | 'subscription' | 'package'

/** The first message of each kind, in the user's words. */
export const ASK_DRAFTS: Record<AskKind, string> = {
  automation:
    'Please set up an automation for me. The work I want done again is:',
  schedule:
    'Please set up a schedule for me. Run it every weekday at 08:00 and do:',
  subscription:
    'Please watch for something and act on it for me. The thing to watch for is:',
  package:
    'Please write a small software package for me. The job it must do is:',
}

export interface AskAnAgentProps {
  kind: AskKind
  agents: { id: string; name: string }[]
  /** Every channel, so the DM of an agent is found in the roster. */
  channels: DmCandidate[]
  /** Open the DM the reader chose. */
  onOpenChannel: (channelId: string) => void
  /** Write the first message into that channel's composer. */
  onDraft: (channelId: string, text: string) => void
  label?: string
}

/** As much of a channel as the DM lookup reads. */
export interface DmCandidate {
  id: string
  kind: string
  agent_ids?: string[]
  user_member: boolean
}

/** The user's own DM with one agent, or null when the workspace holds
 *  none. A DM two agents opened between themselves carries one of them
 *  too, and the user writes nothing in it (ADR-0003), so the lookup
 *  takes the channel the user is a member of. */
export function directMessageChannel(
  channels: DmCandidate[],
  agentId: string,
): string | null {
  const dm = channels.find(
    (channel) =>
      channel.kind === 'dm' &&
      channel.user_member &&
      (channel.agent_ids ?? []).includes(agentId),
  )
  return dm?.id ?? null
}

export function AskAnAgent({
  kind,
  agents,
  channels,
  onOpenChannel,
  onDraft,
  label = 'Ask a sprite to set one up',
}: AskAnAgentProps) {
  const reachable = agents.filter(
    (agent) => directMessageChannel(channels, agent.id) !== null,
  )

  if (reachable.length === 0) {
    return (
      <p className="automations-ask-empty">
        No sprite has a direct message yet. Open one from the sidebar first.
      </p>
    )
  }

  return (
    <Menu
      label={label}
      trigger={
        <Button size="sm" variant="primary" className="automations-ask">
          {label}
        </Button>
      }
      items={reachable.map((agent) => ({
        label: agent.name,
        onSelect: () => {
          const channelId = directMessageChannel(channels, agent.id)
          if (channelId === null) return
          onDraft(channelId, ASK_DRAFTS[kind])
          onOpenChannel(channelId)
        },
      }))}
    />
  )
}
