import { QueryClientProvider as Provider } from '@tanstack/react-query'
import type { ComponentProps } from 'react'

import { CurrentWorkspaceContext, WorkspaceSelectionContext } from '../workspace'

/** Component tests use one current Workspace without a Session read. */
export function QueryClientProvider({ children, ...props }: ComponentProps<typeof Provider>) {
  const current = {
    id: 'ws-1', name: 'Private Workspace', kind: 'private' as const, role: 'manager' as const,
  }
  return (
    <Provider {...props}>
      <WorkspaceSelectionContext value={{ items: [current], current, select: () => {} }}>
        <CurrentWorkspaceContext value={current.id}>{children}</CurrentWorkspaceContext>
      </WorkspaceSelectionContext>
    </Provider>
  )
}
