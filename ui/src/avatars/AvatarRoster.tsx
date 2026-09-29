import { createContext, useContext, useMemo } from 'react'
import type { ReactNode } from 'react'
import type { AgentDto } from '../api/client'
import type { SpriteAppearance } from './catalog'
const Roster = createContext<ReadonlyMap<string, SpriteAppearance>>(new Map())
export function AvatarRoster({
  agents,
  children,
}: {
  agents: readonly AgentDto[]
  children: ReactNode
}) {
  const appearances = useMemo(
    () => new Map(agents.map((agent) => [agent.id, agent.avatar])),
    [agents],
  )
  return <Roster value={appearances}>{children}</Roster>
}
export function useAgentAppearance(id: string) {
  return useContext(Roster).get(id)
}
