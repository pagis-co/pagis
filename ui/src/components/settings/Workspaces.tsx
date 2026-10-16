import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import type { AdministrationWorkspaceDto, ApiClient } from '../../api/client'
import { Button, Frame, Input, Row, SectionLabel, Select } from '../../primitives'
import { useAdministrationWorkspaces, usePeople, useSetSpendCap, workspacesKey } from '../../queries'
import { money, personLabel } from './People'
import './People.css'

function WorkspaceRow({ api, workspace }: { api: ApiClient; workspace: AdministrationWorkspaceDto }) {
  const setCap = useSetSpendCap(api)
  const [cap, setCapValue] = useState(workspace.monthly_spend_cap_usd == null ? '' : String(workspace.monthly_spend_cap_usd))
  const noCap = cap.trim() === ''
  const amount = Number(cap)
  const valid = noCap || (Number.isFinite(amount) && amount >= 0)
  return (
    <div className="people-row">
      <Row className="people-line">
        <span className="people-name">{workspace.name}</span>
        <span>{workspace.person ? personLabel(workspace.person) : `Managers: ${workspace.managers.map(personLabel).join(', ')}`}</span>
        <span>{workspace.person?.email}</span>
        <span className="people-spend">{money(workspace.total.cost_usd)} this month</span>
      </Row>
      <Row className="people-line people-controls">
        <Input type="number" min={0} step="0.01" className="people-cap"
          aria-label={`Monthly spend cap for ${workspace.name}`} placeholder="No cap"
          value={cap} onChange={(event) => setCapValue(event.target.value)} />
        <span className="people-unit">per month</span>
        <Button size="sm" aria-label={`Save the spend cap for ${workspace.name}`}
          disabled={!valid || setCap.isPending}
          onClick={() => setCap.mutate({ workspaceId: workspace.id, capUsd: noCap ? null : amount })}>
          Save cap
        </Button>
      </Row>
      {setCap.isError && <span role="alert" className="settings-error">The Spend Cap could not be saved.</span>}
    </div>
  )
}

export function Workspaces({ api }: { api: ApiClient }) {
  const workspaces = useAdministrationWorkspaces(api)
  const people = usePeople(api)
  const client = useQueryClient()
  const [name, setName] = useState('')
  const [managerId, setManagerId] = useState('')
  const create = useMutation({
    mutationFn: async () => {
      const { data, error } = await api.POST('/api/v1/administration/workspaces', {
        body: { name: name.trim(), manager_id: managerId },
      })
      if (!data) throw new Error(error?.error.message ?? 'Could not create the Shared Workspace.')
    },
    onSuccess: () => {
      setName('')
      void client.invalidateQueries({ queryKey: workspacesKey })
    },
  })
  return (
    <section className="settings-section people">
      <div className="people-title"><h1>Workspaces</h1><span>Set the monthly Spend Cap for each Workspace.</span></div>
      <SectionLabel>Create a Shared Workspace</SectionLabel>
      <form onSubmit={(event) => {
        event.preventDefault()
        create.mutate()
      }}>
        <Input aria-label="Shared Workspace name" value={name} onChange={(event) => setName(event.target.value)} />
        <Select label="First Manager" value={managerId} onValueChange={setManagerId}
          items={(people.data ?? []).filter((person) => !person.disabled).map((person) => ({
            value: person.id,
            label: personLabel(person),
          }))} />
        <Button type="submit" disabled={!name.trim() || !managerId || create.isPending}>Create Shared Workspace</Button>
        {create.isError && <p role="alert">{create.error.message}</p>}
      </form>
      <SectionLabel>Workspaces</SectionLabel>
      <Frame hint="An empty field means no cap. Each Workspace counts its own spend in the calendar month of its timezone.">
        {(workspaces.data ?? []).map((workspace) => <WorkspaceRow key={workspace.id} api={api} workspace={workspace} />)}
        {workspaces.isError && <Row><span role="alert">The Workspaces could not be read.</span></Row>}
      </Frame>
    </section>
  )
}
