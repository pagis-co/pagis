import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type { ApiClient, MailVisibility, SharedWorkspaceLimits } from '../../api/client'
import { Button, Input, Row, Select } from '../../primitives'
import { workspacesKey } from '../../queries'

const ids = (value: string) => [...new Set(value.split(',').map((id) => id.trim()).filter(Boolean))]

function Editor({ api, workspaceId, name, limits, outsidePlugins, outsideConnections }: {
  api: ApiClient; workspaceId: string; name: string; limits: SharedWorkspaceLimits; outsidePlugins: number; outsideConnections: number
}) {
  const client = useQueryClient()
  const [allPlugins, setAllPlugins] = useState(limits.permitted_plugins == null)
  const [allConnections, setAllConnections] = useState(limits.permitted_connections == null)
  const [plugins, setPlugins] = useState(limits.permitted_plugins?.join(', ') ?? '')
  const [connections, setConnections] = useState(limits.permitted_connections?.join(', ') ?? '')
  const [visibility, setVisibility] = useState<MailVisibility>(limits.max_mail_visibility ?? 'all_mail')
  const save = useMutation({
    mutationFn: async () => {
      const { data, error } = await api.PUT('/api/v1/administration/workspaces/{workspace_id}/limits', {
        params: { path: { workspace_id: workspaceId } },
        body: { permitted_plugins: allPlugins ? null : ids(plugins), permitted_connections: allConnections ? null : ids(connections), max_mail_visibility: visibility },
      })
      if (!data) throw new Error(error?.error.message ?? 'The Shared Workspace Limits could not be saved.')
    },
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ['workspace-limits', workspaceId] })
      void client.invalidateQueries({ queryKey: workspacesKey })
      void client.invalidateQueries({ queryKey: ['administration', 'audit'] })
    },
  })
  return <div>
    <Row><label><input type="checkbox" aria-label={`Permit all Plugins for ${name}`} checked={allPlugins} onChange={(event) => setAllPlugins(event.target.checked)} />Permit all Plugins</label>
      <Input aria-label={`Permitted Plugins for ${name}`} placeholder="Plugin ids, separated by commas" disabled={allPlugins} value={plugins} onChange={(event) => setPlugins(event.target.value)} /></Row>
    <Row><label><input type="checkbox" aria-label={`Permit all Connections for ${name}`} checked={allConnections} onChange={(event) => setAllConnections(event.target.checked)} />Permit all Connections</label>
      <Input aria-label={`Permitted Connections for ${name}`} placeholder="Connection ids, separated by commas" disabled={allConnections} value={connections} onChange={(event) => setConnections(event.target.value)} /></Row>
    <Row><Select label={`Highest Mail Visibility for ${name}`} value={visibility} onValueChange={(value) => setVisibility(value as MailVisibility)} items={[{ value: 'agent_only', label: 'Sprite only' }, { value: 'escalations', label: 'Escalations' }, { value: 'all_mail', label: 'All mail' }]} /></Row>
    <p>A cleared permission list permits none. Grants outside the limits cannot be used.</p>
    <p>{outsidePlugins} Plugin Grants and {outsideConnections} Connection Grants are outside the limits.</p>
    <Button aria-label={`Save limits for ${name}`} disabled={save.isPending} onClick={() => save.mutate()}>Save limits</Button>
    {save.isError && <p role="alert">{save.error.message}</p>}
  </div>
}

export function WorkspaceLimits({ api, workspaceId, name }: { api: ApiClient; workspaceId: string; name: string }) {
  const query = useQuery({
    queryKey: ['workspace-limits', workspaceId],
    queryFn: async () => {
      const { data, error } = await api.GET('/api/v1/administration/workspaces/{workspace_id}/limits', { params: { path: { workspace_id: workspaceId } } })
      if (!data) throw new Error(error?.error.message ?? 'The Shared Workspace Limits could not be read.')
      return data
    },
  })
  if (query.isError) return <p role="alert">{query.error.message}</p>
  if (!query.data) return <p>Read Shared Workspace Limits…</p>
  return <Editor api={api} workspaceId={workspaceId} name={name} limits={query.data} outsidePlugins={query.data.outside_plugins} outsideConnections={query.data.outside_connections} />
}
