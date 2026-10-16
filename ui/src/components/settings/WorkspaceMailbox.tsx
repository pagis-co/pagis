import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState, type FormEvent } from 'react'

import type { ApiClient } from '../../api/client'
import { Button, Input, Select } from '../../primitives'
import { useWorkspaceMailbox, workspaceMailKey } from '../../queries'
import { useCurrentWorkspaceId } from '../../workspace'

export function WorkspaceMailbox({ api }: { api: ApiClient }) {
  const workspaceId = useCurrentWorkspaceId()
  const client = useQueryClient()
  const page = useWorkspaceMailbox(api)
  const [connection, setConnection] = useState('')
  const [localPart, setLocalPart] = useState('')
  const [password, setPassword] = useState('')
  const offer =
    page.data?.offers.find((item) => item.connection_id === connection) ??
    page.data?.offers[0]
  const create = useMutation({
    mutationFn: async () => {
      if (!offer) {
        throw new Error('An Administrator sets up the Org mail domain.')
      }
      const { data, error } = await api.POST('/api/v1/workspace/mailbox', {
        body: {
          connection_id: offer.connection_id,
          local_part: localPart,
          ...(offer.password === 'typed' ? { password } : {}),
        },
      })
      if (!data) {
        throw new Error(
          error?.error.message ?? 'Could not create the Workspace Mailbox.',
        )
      }
      return data
    },
    onSuccess: () => {
      setPassword('')
      void client.invalidateQueries({ queryKey: workspaceMailKey(workspaceId) })
    },
  })

  function submit(event: FormEvent) {
    event.preventDefault()
    create.mutate()
  }

  return (
    <section className="settings-section">
      <h3>Workspace Mailbox</h3>
      {page.isPending && <p>Loading Workspace Mailbox…</p>}
      {page.isError && (
        <p role="alert">Could not load the Workspace Mailbox.</p>
      )}
      {page.data?.mailbox ? (
        <>
          <p>{page.data.mailbox.address}</p>
          <p>Spam filtering uses score-only mode. Messages stay in their folders.</p>
        </>
      ) : (
        page.data && (
          <>
            {!offer ? (
              <p>An Administrator sets up the Org mail domain.</p>
            ) : (
              <form onSubmit={submit}>
                {page.data.offers.length > 1 && (
                  <Select
                    label="Mail domain"
                    value={offer.connection_id}
                    onValueChange={setConnection}
                    items={page.data.offers.map((item) => ({
                      value: item.connection_id,
                      label: item.domain,
                    }))}
                  />
                )}
                <label>
                  Local part
                  <Input
                    required
                    value={localPart}
                    onChange={(event) => setLocalPart(event.target.value)}
                  />
                </label>
                <span>@{offer.domain}</span>
                {offer.password === 'typed' && (
                  <>
                    <label>
                      Mailbox password
                      <Input
                        required
                        type="password"
                        autoComplete="new-password"
                        value={password}
                        onChange={(event) => setPassword(event.target.value)}
                      />
                    </label>
                    <p>Create this mailbox at your mail host first.</p>
                  </>
                )}
                <Button
                  type="submit"
                  disabled={create.isPending || !localPart.trim()}
                >
                  Create Workspace Mailbox
                </Button>
              </form>
            )}
            {create.isError && <p role="alert">{create.error.message}</p>}
          </>
        )
      )}
    </section>
  )
}
