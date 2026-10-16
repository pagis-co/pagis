import { useState } from 'react'
import type { ApiClient } from '../../api/client'
import { useIsMobile } from '../../primitives/useMediaQuery'
import { Button, Input } from '../../primitives'
import { errorMessage, useMailFolderMutation } from '../../queries'

export function FolderManager({
  api,
  folders,
}: {
  api: ApiClient
  folders: string[]
}) {
  const phone = useIsMobile()
  const Container = phone ? 'div' : 'details'
  const [name, setName] = useState('')
  const mutation = useMailFolderMutation(api)
  return (
    <Container>
      {!phone && <summary>Manage folders</summary>}
      <form
        onSubmit={(event) => {
          event.preventDefault()
          mutation.mutate(
            { action: 'create', name },
            { onSuccess: () => setName('') },
          )
        }}
      >
        <label>
          Folder name
          <Input
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <Button type="submit" disabled={!name.trim() || mutation.isPending}>
          Create folder
        </Button>
      </form>
      <ul>
        {folders.map((folder) => (
          <FolderItem key={folder} api={api} name={folder} />
        ))}
      </ul>
      {mutation.isError && <p role="alert">Could not create the folder.</p>}
    </Container>
  )
}

function FolderItem({ api, name }: { api: ApiClient; name: string }) {
  const [newName, setNewName] = useState(name)
  const [confirmDelete, setConfirmDelete] = useState(false)
  const mutation = useMailFolderMutation(api)
  const system = [
    'inbox',
    'sent',
    'sent messages',
    'sent items',
    'drafts',
    'junk',
    'trash',
  ].includes(name.toLowerCase())
  return (
    <li>
      {system ? (
        <span>{name}</span>
      ) : (
        <>
          <label>
            Rename {name}
            <Input
              value={newName}
              onChange={(event) => setNewName(event.target.value)}
            />
          </label>
          <Button
            disabled={mutation.isPending || !newName.trim() || newName === name}
            onClick={() => mutation.mutate({ action: 'rename', name, newName })}
          >
            Rename {name}
          </Button>
          <Button
            disabled={mutation.isPending}
            onClick={() => setConfirmDelete(true)}
          >
            Delete {name}
          </Button>
          {confirmDelete && (
            <div role="group" aria-label={`Delete ${name}`}>
              <p>Delete {name} and all mail in this folder?</p>
              <Button
                disabled={mutation.isPending}
                onClick={() => mutation.mutate({ action: 'delete', name })}
              >
                Confirm delete {name}
              </Button>
              <Button onClick={() => setConfirmDelete(false)}>Cancel</Button>
            </div>
          )}
        </>
      )}
      {mutation.isError && (
        <p role="alert">
          {errorMessage(mutation.error, 'Could not change the folder. System folders cannot change.')}
        </p>
      )}
    </li>
  )
}
