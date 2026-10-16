import { useContext } from 'react'
import { useNavigate } from '@tanstack/react-router'

import type { NeedsYouItem } from '../../api/client'
import { Button } from '../../primitives'
import { WorkspaceSelectionContext } from '../../workspace'

/** Open the Request in its owning Shared Workspace before reading it. */
export function SharedRequest({ item }: {
  item: Extract<NeedsYouItem, { kind: 'approval' }>
}) {
  const { items, select } = useContext(WorkspaceSelectionContext)
  const navigate = useNavigate()
  const workspace = items.find((choice) => choice.id === item.workspace_id)
  return (
    <div className="home-shared-request">
      <p>{item.line}</p>
      <strong>{item.title}</strong>
      <p>{item.body}</p>
      <Button
        onClick={() => {
          select(item.workspace_id)
          void navigate({
            to: '/',
            search: (previous) => ({ ...previous, request: item.request_id }),
          })
        }}
      >
        Open in {workspace?.name ?? 'Shared Workspace'}
      </Button>
    </div>
  )
}
