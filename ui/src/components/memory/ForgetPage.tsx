// Forget this page. A Subject Page is what its source items
// said, so the page goes when its items go: each item on the Timeline
// gets the Forget flow of a connection page (preview, confirm,
// retry, allow again). Forget blocks the reimport; a file delete would
// not.

import { useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'

import type { ApiClient } from '../../api/client'
import { ActionSheet, Button, Dialog } from '../../primitives'
import { useIsMobile } from '../../state/useIsMobile'
import { ForgetControl } from '../ForgetControl'
import { parseSourceReference, type TimelineEntry } from './subjectPage'

interface SourceItem {
  key: string
  resource: string
  connectionId: string
  itemId: string
}

/** The distinct source items of a Timeline, in Timeline order. */
function sourceItems(timeline: readonly TimelineEntry[]): SourceItem[] {
  const items = new Map<string, SourceItem>()
  for (const entry of timeline) {
    const { resource, connectionId, itemId } = parseSourceReference(entry.sourceReference)
    if (connectionId === null || itemId === null) continue
    const key = `${resource}:${connectionId}:${itemId}`
    if (!items.has(key)) items.set(key, { key, resource, connectionId, itemId })
  }
  return [...items.values()]
}

export function ForgetPage({
  api,
  workspaceId,
  title,
  timeline,
}: {
  api: ApiClient
  workspaceId: string
  title: string
  timeline: readonly TimelineEntry[]
}) {
  const phone = useIsMobile()
  const [open, setOpen] = useState(false)
  const queryClient = useQueryClient()
  const items = sourceItems(timeline)
  if (items.length === 0) return null

  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: ['forget-operations'] })
    await queryClient.invalidateQueries({ queryKey: ['memory-pages'] })
    await queryClient.invalidateQueries({ queryKey: ['memory-feed'] })
    await queryClient.resetQueries({ queryKey: ['memory-file'] })
  }

  const description = `${title} comes from ${items.length} source ${items.length === 1 ? 'item' : 'items'}. Forget each item to remove what it wrote and to stop a new import of it.`
  const controls = <>      {items.map((item) => {
        const target = {
          kind: 'source' as const,
          source: {
            workspace_id: workspaceId,
            connection_id: item.connectionId,
            resource: item.resource,
          },
          source_id: item.itemId,
        }
        return (
          <ForgetControl
            key={item.key}
            label={`${item.resource} item ${item.itemId}`}
            preview={async () => {
              const response = await api.POST('/api/v1/knowledge/forget/preview', {
                body: { target },
              })
              if (response.error) throw response.error
              return response.data
            }}
            confirm={async (preview) => {
              const response = await api.POST('/api/v1/knowledge/forget', {
                body: { target, preview },
              })
              if (response.error) throw response.error
              await refresh()
              return response.data
            }}
            retry={async (id) => {
              const response = await api.POST('/api/v1/knowledge/forget/{id}/retry', {
                params: { path: { id } },
              })
              if (response.error) throw response.error
              await refresh()
            }}
            reopt={async (id) => {
              const response = await api.POST('/api/v1/knowledge/forget/{id}/reopt', {
                params: { path: { id } },
              })
              if (response.error) throw response.error
              await refresh()
            }}
          />
        )
      })}</>
  if (phone) return <><Button variant="danger-quiet" onClick={() => setOpen(true)}>Forget this page</Button><ActionSheet open={open} onOpenChange={setOpen} title="Forget this page" description={description} cancelLabel="Done">{controls}</ActionSheet></>

  return (
    <Dialog
      open={open}
      onOpenChange={setOpen}
      title="Forget this page"
      description={description}
      trigger={
        <Button variant="ghost" size="sm">
          Forget this page
        </Button>
      }
    >
      {controls}
    </Dialog>
  )
}
