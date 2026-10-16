import { createContext, useContext, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'

import type { ApiClient } from './api/client'

export const privateWorkspaceKey = ['private-workspace'] as const

/** The Private Workspace belongs to the Session. */
export function useWorkspace(api: ApiClient) {
  return useQuery({
    queryKey: privateWorkspaceKey,
    queryFn: async () => {
      const { data, error } = await api.GET('/api/v1/workspace')
      if (data === undefined) throw error ?? new Error('request failed')
      return data
    },
    staleTime: Infinity,
  })
}

export const CurrentWorkspaceContext = createContext<string | null>(null)

export function CurrentWorkspaceProvider({ api, children }: { api: ApiClient; children: ReactNode }) {
  const workspace = useWorkspace(api)
  if (workspace.isError) return <p role="alert">Could not load the Workspace.</p>
  if (workspace.data === undefined) return null
  return <CurrentWorkspaceContext value={workspace.data.id}>{children}</CurrentWorkspaceContext>
}

export function useCurrentWorkspaceId(): string {
  const id = useContext(CurrentWorkspaceContext)
  if (id === null) throw new Error('CurrentWorkspaceProvider is required')
  return id
}
