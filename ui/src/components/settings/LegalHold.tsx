import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { AdministrationWorkspaceDto, ApiClient } from '../../api/client'
import { Badge, Button, Frame, Input, Row, SectionLabel } from '../../primitives'
import { workspacesKey } from '../../queries'

const auditKey = ['administration', 'audit'] as const

export function LegalHoldControl({ api, workspace }: { api: ApiClient; workspace: AdministrationWorkspaceDto }) {
  const [reason, setReason] = useState('')
  const client = useQueryClient()
  const held = workspace.legal_hold != null
  const change = useMutation({
    mutationFn: async () => {
      const params = { path: { workspace_id: workspace.id } }
      const response = held
        ? await api.DELETE('/api/v1/admin/workspaces/{workspace_id}/legal-hold', { params })
        : await api.PUT('/api/v1/admin/workspaces/{workspace_id}/legal-hold', { params, body: { reason: reason.trim() } })
      if (response.error) throw new Error(response.error.error.message)
    },
    onSuccess: () => {
      setReason('')
      void client.invalidateQueries({ queryKey: workspacesKey })
      void client.invalidateQueries({ queryKey: auditKey })
    },
  })
  return <div>
    {held && <Badge>Under Legal Hold</Badge>}
    <p>A Legal Hold preserves this Workspace. Nothing in it can be deleted until the hold ends.</p>
    <form onSubmit={(event) => { event.preventDefault(); change.mutate() }}>
      <Row className="people-line people-controls">
        {!held && <Input required aria-label={`Legal Hold reason for ${workspace.name}`} placeholder="Reason for the Legal Hold"
          value={reason} onChange={(event) => setReason(event.target.value)} />}
        <Button type="submit" size="sm" disabled={change.isPending || (!held && !reason.trim())}
          aria-label={`${held ? 'End' : 'Start'} Legal Hold for ${workspace.name}`}>
          {held ? 'End Legal Hold' : 'Start Legal Hold'}
        </Button>
      </Row>
      {change.isError && <p role="alert">{change.error.message}</p>}
    </form>
  </div>
}

export function AuditLog({ api }: { api: ApiClient }) {
  const [offset, setOffset] = useState(0)
  const audit = useQuery({
    queryKey: [...auditKey, offset],
    queryFn: async () => {
      const { data, error } = await api.GET('/api/v1/admin/audit', { params: { query: { offset, limit: 25 } } })
      if (!data) throw new Error(error?.error.message ?? 'The Audit Log could not be read.')
      return data
    },
  })
  return <>
    <SectionLabel>Audit Log</SectionLabel>
    <Frame>
      {(audit.data?.items ?? []).map((entry) => <Row key={entry.id} className="people-line audit-entry">
        <time dateTime={new Date(entry.at).toISOString()}>{new Date(entry.at).toLocaleString()}</time>
        <span>{entry.action}</span>
        <span>Workspace: {entry.subject_workspace_id}</span>
        <span>Author: {entry.actor.kind === 'person' ? entry.actor.person_id : entry.actor.kind === 'agent' ? entry.actor.agent_id : 'System'}</span>
        {entry.reason && <span>{entry.reason}</span>}
      </Row>)}
      {audit.isError && <Row><span role="alert">{audit.error.message}</span></Row>}
      {audit.data?.items.length === 0 && <Row>No Audit Entries.</Row>}
    </Frame>
    <Row>
      <Button disabled={offset === 0 || audit.isFetching} onClick={() => setOffset(Math.max(0, offset - 25))}>Previous entries</Button>
      <Button disabled={audit.data?.next_offset == null || audit.isFetching} onClick={() => setOffset(audit.data?.next_offset ?? offset)}>Older entries</Button>
    </Row>
  </>
}
