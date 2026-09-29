import { useEffect, useState } from 'react'
import { create } from 'zustand'
import type { AvatarReaction } from './reactions'
import { avatarMotion } from './motion'
import type { Presence } from '../primitives/avatar'
import { useConnection } from '../state/stores'
import { usePresence } from '../state/presence'

export const useAvatarReaction = create<{
  reaction: (AvatarReaction & { until: number }) | null
}>(() => ({ reaction: null }))
export function useLiveAvatarMotion(
  agentId: string,
  presence: Presence,
  hover = false,
) {
  const online = useConnection((state) => state.status === 'online')
  const channel = usePresence((state) => state.selectedChannelId)
  const reaction = useAvatarReaction((state) => state.reaction)
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    setNow(Date.now())
    if (!reaction) return
    const timer = setTimeout(
      () => setNow(Date.now()),
      Math.max(0, reaction.until - Date.now()) + 1,
    )
    return () => clearTimeout(timer)
  }, [reaction])
  const motion = avatarMotion({ presence, online, hover })
  if (
    online &&
    presence !== 'working' &&
    presence !== 'waiting' &&
    presence !== 'oncall' &&
    reaction?.agentId === agentId &&
    reaction.channelId === channel &&
    reaction.until > now
  ) {
    return { ...motion, clip: reaction.clip, expression: reaction.expression }
  }
  return motion
}
