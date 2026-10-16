import { fireEvent, screen, waitFor } from '@testing-library/react'
import { expect, it, vi } from 'vitest'

import { WorkspaceSelectionContext } from '../../workspace'
import { renderInRouter } from '../../test/router'
import { SharedRequest } from './SharedRequest'

it('selects the owning Shared Workspace before opening its Request', async () => {
  const select = vi.fn()
  const privateWorkspace = {
    id: 'private', name: 'Private Workspace', kind: 'private' as const, role: 'manager' as const,
  }
  const team = { id: 'support', name: 'Support', kind: 'shared' as const, role: 'responder' as const }
  const history = renderInRouter(
    <WorkspaceSelectionContext
      value={{ items: [privateWorkspace, team], current: privateWorkspace, select }}
    >
      <SharedRequest item={{
        kind: 'approval', workspace_id: team.id, id: 'request:r1', agent_id: 'aria',
        request_id: 'r1', request_kind: 'tool_action', line: 'Aria needs your approval',
        title: 'Send the answer', body: 'Answer the support question', url: '/', at: 1,
      }} />
    </WorkspaceSelectionContext>,
  )
  fireEvent.click(await screen.findByRole('button', { name: 'Open in Support' }))
  expect(select).toHaveBeenCalledWith('support')
  await waitFor(() => expect(history.location.search).toContain('request=r1'))
})
