import { Select } from './primitives'
import { createContext, useContext, useState, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'

import type { ApiClient } from './api/client'
import { selectWorkspace } from './api/workspace-path'

export const CurrentWorkspaceContext = createContext<string | null>(null)

type WorkspaceRole = 'manager' | 'responder' | 'reader'
type WorkspaceChoice = { id: string; name: string; kind: 'private' | 'shared'; role: WorkspaceRole }
export const WorkspaceSelectionContext = createContext<{
  items: WorkspaceChoice[]
  current: WorkspaceChoice | undefined
  select: (id: string) => void
}>({ items: [], current: undefined, select: () => {} })

export function CurrentWorkspaceProvider({ api, children }: { api: ApiClient; children: ReactNode }) {
  const [selected, setSelected] = useState<string | null>(null)
  const choices = useQuery({
    queryKey: ['workspaces'],
    refetchInterval: 30_000,
    queryFn: async () => {
      const { data, error } = await api.GET('/api/v1/workspaces')
      if (data === undefined) throw error ?? new Error('request failed')
      return data.items
    },
  })
  if (choices.isError) return <p role="alert">Could not load the Workspace.</p>
  if (choices.data === undefined) return null
  const items = choices.data
  const privateWorkspace = items.find(item => item.kind === 'private')
  if (privateWorkspace === undefined) return <p role="alert">Could not load the Private Workspace.</p>
  const id = selected !== null && items.some((item) => item.id === selected)
    ? selected
    : privateWorkspace.id
  const current = items.find((item) => item.id === id)
  selectWorkspace(id === privateWorkspace.id ? null : id)
  return (
    <WorkspaceSelectionContext value={{ items, current, select: setSelected }}>
      <CurrentWorkspaceContext value={id}>{children}</CurrentWorkspaceContext>
    </WorkspaceSelectionContext>
  )
}

export function useWorkspaceAccess() {
  const { current } = useContext(WorkspaceSelectionContext)
  return {
    isPrivate: current?.kind !== 'shared',
    canManage: current?.kind !== 'shared' || current.role === 'manager',
    canRespond: current?.role !== 'reader',
    canManageMembers: current?.kind === 'shared' && current.role === 'manager',
  }
}

export function useWorkspace(api: ApiClient) {
  const workspaceId = useCurrentWorkspaceId()
  return useQuery({
    queryKey: ['workspace', workspaceId, 'details'],
    queryFn: async () => {
      const { data, error } = await api.GET('/api/v1/workspace')
      if (data === undefined) throw error ?? new Error('request failed')
      return data
    },
  })
}

export function WorkspaceSwitcher({ onSwitch }: { onSwitch?: () => void }) {
  const { items, select } = useContext(WorkspaceSelectionContext)
  const id = useCurrentWorkspaceId()
  return (
    <Select
      label="Workspace"
      value={id}
      onValueChange={(value) => {
        select(value)
        onSwitch?.()
      }}
      items={items.map((item) => ({
        value: item.id,
        label: item.kind === 'private' ? 'Private Workspace' : item.name,
      }))}
    />
  )
}

export function useCurrentWorkspaceId(): string {
  const id = useContext(CurrentWorkspaceContext)
  if (id === null) throw new Error('CurrentWorkspaceProvider is required')
  return id
}
