import { useState } from 'react'
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
  onMove,
}: {
  api: ApiClient
  id: string
  labels: string[]
  onMove?: () => void
}) {
  const [label, setLabel] = useState('')
  const [folder, setFolder] = useState('')
  const folders = useWorkspaceMailFolders(api)
  const mutation = useOrganizeWorkspaceMail(api, id)
  return (
    <div className="mail-controls">
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
