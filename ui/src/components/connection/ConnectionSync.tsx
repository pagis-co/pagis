// The Sync section of a connection page: a status strip that
// says how the acquisition goes, then the two settings that steer it.
// Every control saves at once, so the section has no Save of its own.

import type { components } from '../../api/schema'
import type { ApiClient } from '../../api/client'
import { Badge, Button, Frame, Input, Row, Select } from '../../primitives'
import { errorMessage, useAccountSync, useConfigureSync } from '../../queries'

import './connection.css'

type SyncStatus = components['schemas']['SyncStatus']
type Agent = { id: string; name: string; status: string }

/** The one word the strip leads with. The order is the order the user
 *  must act in: a failure first, then the work, then the calm state. */
export function stateWord(status: SyncStatus): string {
  if (!status.config.enabled) return 'Paused'
  if (status.acquisition_error != null) return 'Acquisition needs attention'
  if (status.arrival_error != null) return 'Arrival delivery needs attention'
  if (status.arrival_pending > 0) return 'Delivering arrivals'
  if (!status.caught_up) return 'Importing history'
  return 'Up to date'
}

function tone(status: SyncStatus): 'working' | 'waiting' | 'failed' | 'neutral' {
  if (!status.config.enabled) return 'neutral'
  if (status.acquisition_error != null || status.arrival_error != null) return 'failed'
  if (status.arrival_pending > 0 || !status.caught_up) return 'waiting'
  return 'working'
}

/** How long ago a moment is, in the largest unit that fits. */
export function timeAgo(at: number, now: number = Date.now()): string {
  const minutes = Math.max(0, Math.round((now - at) / 60_000))
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours} h ago`
  return `${Math.round(hours / 24)} days ago`
}

function dateValue(since: number): string {
  return since === 0 ? '' : new Date(since).toISOString().slice(0, 10)
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <span className="connection-stat">
      <span className="connection-stat-label">{label}</span>
      <span className="connection-stat-value">{value}</span>
    </span>
  )
}

export function ConnectionSync({
  api,
  connectionId,
  agents,
}: {
  api: ApiClient
  connectionId: string
  agents: Agent[]
}) {
  const query = useAccountSync(api, connectionId)
  const save = useConfigureSync(api, connectionId)
  const status = query.data?.status
  if (status == null) {
    return (
      <Frame>
        <Row>
          <span className="connection-hint">
            {query.isPending ? 'Reading the sync…' : 'This account does not sync yet.'}
          </span>
        </Row>
      </Frame>
    )
  }
  const config = status.config
  const configure = (change: Partial<components['schemas']['ConfigureSync']>) =>
    save.mutate({
      agent_id: config.agent_id,
      enabled: config.enabled,
      since: config.since,
      filter: config.filter,
      ...change,
    })

  return (
    <Frame>
      <Row className="connection-sync-strip">
        <Badge tone={tone(status)}>{stateWord(status)}</Badge>
        <Stat label="Acquired" value={`${status.processed.toLocaleString()} messages`} />
        <Stat label="Last arrival" value={timeAgo(status.updated_at)} />
        <Stat
          label="Arrivals today"
          value={`${status.arrival_processed} handled · ${status.arrival_skipped} skipped`}
        />
        <span className="connection-row-spacer" />
        <Button
          disabled={save.isPending}
          onClick={() => configure({ enabled: !config.enabled })}
        >
          {config.enabled ? 'Pause sync' : 'Resume sync'}
        </Button>
      </Row>
      <Row>
        <span className="connection-row-label">Responsible sprite</span>
        <Select
          label="Responsible sprite"
          value={config.agent_id}
          onValueChange={(agent_id) => configure({ agent_id })}
          items={agents
            .filter((agent) => agent.status !== 'archived')
            .map((agent) => ({ value: agent.id, label: agent.name }))}
        />
        <span className="connection-hint">
          runs the reflections and gets the arrivals
        </span>
      </Row>
      <Row>
        <span className="connection-row-label">Import history from</span>
        <Input
          type="date"
          aria-label="Import history from"
          max={new Date().toISOString().slice(0, 10)}
          value={dateValue(config.since)}
          onChange={(event) =>
            configure({
              since: event.target.value === '' ? 0 : Date.parse(event.target.value),
            })
          }
        />
        <span className="connection-hint">
          blank imports everything; a new date starts a new scan
        </span>
      </Row>
      {query.data?.blocked_reason != null && (
        <Row>
          <p className="connection-error" role="alert">
            {query.data.blocked_reason}
          </p>
        </Row>
      )}
      {save.isError && (
        <Row>
          <p className="connection-error" role="alert">
            {errorMessage(save.error, 'Those sync settings could not be kept.')}
          </p>
        </Row>
      )}
    </Frame>
  )
}
