// One Connection on the list. The row says who the
// account is, what state it is in, and the two acts the list owns:
// the way in to the connection's own page, and the delete. Everything
// else about a Connection — the Grants, the sync, the Forget — lives
// on that page, so the list stays one line per account.

import type { ReactNode } from 'react'

import type { ApiClient, ConnectionDto } from '../../api/client'
import { Badge, Button, Row } from '../../primitives'
import { CONNECTION_STATE } from '../ConnectionStatus'
import { SyncLine } from './SyncLine'

import '../settings.css'

/** What the row says under the name: what kind of thing this is or the
 *  account it signs in as, then the alias a tool call picks it by. A
 *  provider id is not a word the user chose, so it never shows: the
 *  catalogue's own label stands in for an account-less Connection. */
function detail(connection: ConnectionDto, kind: string | undefined): string {
  const who = connection.account ?? kind ?? connection.provider
  return `${who} · known as ${connection.alias}`
}

export function ConnectionRow({
  api,
  connection,
  kind,
  onOpen,
  onDelete,
  deleting,
  testId,
  children,
}: {
  api: ApiClient
  connection: ConnectionDto
  /** What this Connection is, in the catalogue's words. */
  kind?: string
  /** The hook a test reads the whole row by. */
  testId?: string
  onOpen: () => void
  /** The delete, or `undefined` for an Installation Connection, which
   *  the Administration Interface removes. */
  onDelete?: () => void
  deleting: boolean
  /** What this provider adds under the row's own line, if anything. */
  children?: ReactNode
}) {
  const state = CONNECTION_STATE[connection.status] ?? {
    label: connection.status,
    tone: 'failed' as const,
  }
  return (
    <Row className="connection-row" data-testid={testId}>
      <div className="connection-row-line">
        <span className="settings-tile" aria-hidden>
          {connection.display_name.charAt(0).toUpperCase()}
        </span>
        <span className="settings-row-name">
          <strong>{connection.display_name}</strong>
          <span className="settings-row-detail">{detail(connection, kind)}</span>
        </span>
        <span className="settings-row-aside">
          <SyncLine api={api} connectionId={connection.id} />
          <Badge tone={state.tone}>{state.label}</Badge>
        </span>
        <Button
          size="sm"
          aria-label={`Open ${connection.display_name}`}
          onClick={onOpen}
        >
          Open
        </Button>
        {onDelete !== undefined && (
          <Button
            variant="danger-quiet"
            size="sm"
            aria-label={`Delete ${connection.display_name}`}
            disabled={deleting}
            onClick={onDelete}
          >
            Delete
          </Button>
        )}
      </div>
      {children}
    </Row>
  )
}
