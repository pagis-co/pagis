import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ApiClient } from '../api/client'
import type { components } from '../api/schema'
import { Button, Frame, SectionLabel, Select } from '../primitives'
import { useAdministrationWorkspaces } from '../queries'

type Defaults = components['schemas']['PersonDefaults']
const key = ['administration', 'person-defaults']

function DefaultsForm({ api, initial, offers }: {
  api: ApiClient
  initial: Defaults
  offers: components['schemas']['MailboxOfferDto'][]
}) {
  const [defaults, setDefaults] = useState(initial)
  const workspaces = useAdministrationWorkspaces(api)
  const client = useQueryClient()
  const save = useMutation({
    mutationFn: async () => {
      const { error } = await api.PUT('/api/v1/administration/person-defaults', { body: defaults })
      if (error) throw new Error(error.error.message)
    },
    onSuccess: () => { void client.invalidateQueries({ queryKey: key }) },
  })
  return <form onSubmit={(event) => { event.preventDefault(); save.mutate() }}>
    <Frame hint="A new Person receives a Workspace Mailbox by the address rule and joins these Shared Workspaces. A taken address needs your choice before the account is created.">
      <Select label="Address rule" value={defaults.address_rule ?? 'none'}
        onValueChange={(value) => setDefaults({ ...defaults, address_rule: (value === 'none' ? null : value) as Defaults['address_rule'],
          mailbox_connection_id: value === 'none' ? null : defaults.mailbox_connection_id })}
        items={[
          { value: 'none', label: 'No Workspace Mailbox' },
          { value: 'first_name', label: 'First name' },
          { value: 'first_last', label: 'First name.last name' },
          { value: 'first_initial_last', label: 'First initial + last name' },
        ]} />
      {defaults.address_rule && <Select label="Mailbox Provider"
        value={defaults.mailbox_connection_id ?? ''}
        onValueChange={(value) => setDefaults({ ...defaults, mailbox_connection_id: value })}
        items={offers.map((offer) => ({ value: offer.connection_id, label: offer.domain }))} />}
      {defaults.address_rule && offers.length === 0 && <p>Connect a Mailbox Provider that creates Workspace Mailboxes.</p>}
      <SectionLabel>Default Shared Workspaces</SectionLabel>
      {workspaces.isError && <p role="alert">The Shared Workspaces could not be read.</p>}
      {(workspaces.data ?? []).filter((workspace) => workspace.kind === 'shared').map((workspace) => {
        const member = defaults.shared_workspaces.find((member) => member.workspace_id === workspace.id)
        return <div key={workspace.id}>
          <label><input type="checkbox" aria-label={`Join ${workspace.name}`} checked={!!member}
            onChange={(event) => setDefaults({ ...defaults, shared_workspaces: event.target.checked
              ? [...defaults.shared_workspaces, { workspace_id: workspace.id, role: 'responder' }]
              : defaults.shared_workspaces.filter((member) => member.workspace_id !== workspace.id) })} />{workspace.name}</label>
          {member && <Select label={`Role in ${workspace.name}`} value={member.role}
            onValueChange={(value) => setDefaults({ ...defaults, shared_workspaces: defaults.shared_workspaces.map((item) => item.workspace_id === workspace.id
              ? { ...item, role: value as typeof item.role } : item) })}
            items={[{ value: 'reader', label: 'Reader' }, { value: 'responder', label: 'Responder' }, { value: 'manager', label: 'Manager' }]} />}

        </div>
      })}
      <Button type="submit" disabled={save.isPending || (!!defaults.address_rule && !defaults.mailbox_connection_id)}>Save defaults</Button>
      {save.isError && <p role="alert">{save.error.message}</p>}
      {save.isSuccess && <p role="status">Defaults saved.</p>}
    </Frame>
  </form>
}

export function PersonDefaults({ api }: { api: ApiClient }) {
  const query = useQuery({ queryKey: key, queryFn: async () => {
    const { data, error } = await api.GET('/api/v1/administration/person-defaults')
    if (!data) throw new Error(error?.error.message ?? 'The defaults could not be read.')
    return data
  } })
  return <section className="settings-section">
    <h1>New People</h1>
    {query.isError && <p role="alert">{query.error.message}</p>}
    {query.data && <DefaultsForm api={api} initial={query.data.defaults} offers={query.data.mailbox_offers} />}
  </section>
}
