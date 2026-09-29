// One connection is one page: who reaches it, how it syncs,
// what reflects from it, and how to end it. Each section saves as it
// changes, so the page has no Save of its own.

import { Plug } from 'lucide-react'

import type { ApiClient } from '../../api/client'
import { Button, SectionLabel } from '../../primitives'
import {
  useAccountSync,
  useAgents,
  useConfigureSync,
  useConnections,
  useGrants,
  useSyncCatalogue,
} from '../../queries'
import { PageState } from '../PageState'
import type { ReflectionFilter } from './rules'
import { ConnectionAccess } from './ConnectionAccess'
import { ConnectionForget } from './ConnectionForget'
import { ConnectionHeader } from './ConnectionHeader'
import { ConnectionRules } from './ConnectionRules'
import { ConnectionSync } from './ConnectionSync'

import './connection.css'

export function ConnectionPage({
  api,
  connectionId,
  onBack,
}: {
  api: ApiClient
  connectionId: string
  onBack: () => void
}) {
  const connections = useConnections(api)
  const agents = useAgents(api)
  const grants = useGrants(api)
  const sync = useAccountSync(api, connectionId)
  const catalogue = useSyncCatalogue(api, connectionId)
  const save = useConfigureSync(api, connectionId)
  const connection = (connections.data ?? []).find((item) => item.id === connectionId)
  const status = sync.data?.status

  if (connections.isPending) {
    return <PageState icon={Plug} title="Loading the connection…" />
  }
  if (connection === undefined) {
    return (
      <PageState icon={Plug} title="That connection is gone">
        It was disconnected, or the address names a connection that never was.
        <Button onClick={onBack}>Back to Connections</Button>
      </PageState>
    )
  }

  const saveFilter = (filter: ReflectionFilter) => {
    if (status == null) return
    save.mutate({
      agent_id: status.config.agent_id,
      enabled: status.config.enabled,
      since: status.config.since,
      filter,
    })
  }

  return (
    <div className="connection-page">
      <ConnectionHeader api={api} connection={connection} onBack={onBack} />

      <SectionLabel>Access</SectionLabel>
      <ConnectionAccess
        api={api}
        connection={connection}
        agents={agents.data ?? []}
        grants={grants.data ?? []}
      />

      <SectionLabel>Sync</SectionLabel>
      <ConnectionSync api={api} connectionId={connectionId} agents={agents.data ?? []} />

      {status != null && catalogue.data != null && (
        <>
          <div className="connection-section-head">
            <SectionLabel>What reflects</SectionLabel>
            <span className="connection-hint">
              The first rule that holds decides. A page that no rule holds for
              takes the last line.
            </span>
          </div>
          <ConnectionRules
            api={api}
            connectionId={connectionId}
            catalogue={catalogue.data.catalogue}
            filter={status.config.filter}
            backfill={{
              reflected: status.backfill_reflected,
              pending: status.backfill_pending,
            }}
            onChange={saveFilter}
          />
        </>
      )}

      <SectionLabel>Forget</SectionLabel>
      <ConnectionForget api={api} connectionId={connectionId} />
    </div>
  )
}
