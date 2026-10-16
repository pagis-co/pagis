import { useState } from 'react'
import type { AdministrationWorkspaceDto, ApiClient } from '../../api/client'
import { Button, Frame, Input, Row, SectionLabel } from '../../primitives'
import { useAdministrationWorkspaces, useSetSpendCap } from '../../queries'
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
        <span>{personLabel(workspace.person)}</span>
        <span>{workspace.person.email}</span>
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
  return (
    <section className="settings-section people">
      <div className="people-title"><h1>Workspaces</h1><span>Set the monthly Spend Cap for each Workspace.</span></div>
      <SectionLabel>Workspaces</SectionLabel>
      <Frame hint="An empty field means no cap. Each Workspace counts its own spend in the calendar month of its timezone.">
        {(workspaces.data ?? []).map((workspace) => <WorkspaceRow key={workspace.id} api={api} workspace={workspace} />)}
        {workspaces.isError && <Row><span role="alert">The Workspaces could not be read.</span></Row>}
      </Frame>
    </section>
  )
}
