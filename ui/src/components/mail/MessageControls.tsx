import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useCurrentWorkspaceId } from '../../workspace'
import { needsYouKey, workspaceMailKey } from '../../queries'
import type { ApiClient } from '../../api/client'
import { Button, Input, Select } from '../../primitives'
import {
  useOrganizeWorkspaceMail,
  useWorkspaceMailFolders,
} from '../../queries'

export function MessageControls({
  api,
  id,
  labels,
  needsYou = false,
  onMove,
}: {
  api: ApiClient
  id: string
  labels: string[]
  needsYou?: boolean
  onMove?: () => void
}) {
  const workspace = useCurrentWorkspaceId()
  const client = useQueryClient()
  const correction = useMutation({ mutationFn: async (spam: boolean) => {
    const result = await api.POST('/api/v1/workspace/mail/messages/{message_id}/spam', { params: { path: { message_id: id } }, body: { spam } })
    if (result.error) throw result.error
  }, onSuccess: async () => {
    await client.invalidateQueries({ queryKey: workspaceMailKey(workspace) })
    onMove?.()
  } })
  const done = useMutation({
    mutationFn: async () => {
      const result = await api.POST('/api/v1/workspace/mail/messages/{message_id}/done', {
        params: { path: { message_id: id } },
      })
      if (result.error) throw result.error
    },
    onSuccess: async () => {
      await Promise.all([
        client.invalidateQueries({ queryKey: workspaceMailKey(workspace) }),
        client.invalidateQueries({ queryKey: needsYouKey(workspace) }),
      ])
      onMove?.()
    },
  })
  const [label, setLabel] = useState('')
  const [folder, setFolder] = useState('')
  const folders = useWorkspaceMailFolders(api)
  const mutation = useOrganizeWorkspaceMail(api, id)
  return (
    <div className="mail-controls">
      {needsYou && <Button disabled={done.isPending} onClick={() => done.mutate()}>Done</Button>}
      {done.isError && <p role="alert">Could not mark the message as done. Try again.</p>}
      <Button
        disabled={mutation.isPending}
        onClick={() => mutation.mutate({ action: 'delete' }, { onSuccess: onMove })}
      >
        Delete
      </Button>
      <Button disabled={correction.isPending} onClick={() => correction.mutate(true)}>Spam</Button>
      <Button disabled={correction.isPending} onClick={() => correction.mutate(false)}>Not spam</Button>
      {correction.isError && <p role="alert">Could not send the spam correction.</p>}
      <Select
        label="Move to folder"
        value={folder}
        onValueChange={setFolder}
        items={(folders.data?.items ?? []).map((name) => ({
          value: name,
          label: name,
        }))}
      />
      <Button
        disabled={!folder || mutation.isPending}
        onClick={() =>
          mutation.mutate(
            { action: 'move', name: folder },
            { onSuccess: onMove },
          )
        }
      >
        Move
      </Button>
      <label>
        Label
        <Input
          value={label}
          onChange={(event) => setLabel(event.target.value)}
        />
      </label>
      <Button
        disabled={!label.trim() || mutation.isPending}
        onClick={() =>
          mutation.mutate(
            { action: 'label', name: label },
            { onSuccess: () => setLabel('') },
          )
        }
      >
        Apply label
      </Button>
      {labels.map((name) => (
        <Button
          key={name}
          disabled={mutation.isPending}
          onClick={() => mutation.mutate({ action: 'unlabel', name })}
        >
          Remove label {name}
        </Button>
      ))}
      {mutation.isError && <p role="alert">Could not organize mail.</p>}
    </div>
  )
}
