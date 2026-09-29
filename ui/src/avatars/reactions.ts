import type { EventRow } from '../api/client'
import type { SpriteClip, SpriteExpression } from './motion'
export interface AvatarReaction {
  agentId: string
  channelId: string
  clip: SpriteClip
  expression: SpriteExpression
  seconds: number
}
/** Only fresh foreground events cause a brief expression. History stays quiet. */
export function createAvatarReactions() {
  const seen = new Set<string>()
  return {
    accept(
      event: EventRow,
      context: {
        replay: boolean
        channelId: string | null
        previousRunState?: string
      },
    ): AvatarReaction | null {
      if (seen.has(event.id)) return null
      seen.add(event.id)
      if (seen.size > 256) seen.delete(seen.values().next().value!)
      if (
        context.replay ||
        !event.agent_id ||
        !event.channel_id ||
        event.channel_id !== context.channelId
      )
        return null
      const payload = event.payload as { author_kind?: string; to?: string }
      const identity = { agentId: event.agent_id, channelId: event.channel_id }
      if (
        event.event_type === 'message.completed' &&
        payload.author_kind === 'agent'
      )
        return { ...identity, expression: 'Smile', clip: 'Idle', seconds: 1.6 }
      if (
        event.event_type === 'run.state_changed' &&
        payload.to === 'failed' &&
        context.previousRunState &&
        context.previousRunState !== 'reflecting'
      )
        return { ...identity, expression: 'Concern', clip: 'Error', seconds: 3 }
      return null
    },
  }
}
