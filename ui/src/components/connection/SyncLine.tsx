// The one sync line a Connections card carries: how much the
// account has acquired, when it last heard something, and how many
// rules decide what reflects. The page behind the card holds the rest.

import type { ApiClient } from '../../api/client'
import { useAccountSync } from '../../queries'
import { timeAgo } from './ConnectionSync'

import './connection.css'

export function SyncLine({ api, connectionId }: { api: ApiClient; connectionId: string }) {
  const status = useAccountSync(api, connectionId).data?.status
  if (status == null) return null
  const rules = status.config.filter.rules.length
  return (
    <span className="connection-sync-line">
      <span>
        Sync: {status.processed.toLocaleString()} pages · last{' '}
        {timeAgo(status.updated_at)}
      </span>
      <span>
        Reflection filter: {rules} {rules === 1 ? 'rule' : 'rules'}
      </span>
    </span>
  )
}
