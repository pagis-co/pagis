import { QueryClientProvider as Provider } from '@tanstack/react-query'
import type { ComponentProps } from 'react'

import { CurrentWorkspaceContext } from '../workspace'

/** Component tests use one current Workspace without a Session read. */
export function QueryClientProvider({ children, ...props }: ComponentProps<typeof Provider>) {
  return <Provider {...props}><CurrentWorkspaceContext value="ws-1">{children}</CurrentWorkspaceContext></Provider>
}
