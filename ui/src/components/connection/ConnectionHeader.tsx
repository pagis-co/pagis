// The head of a connection page: who the account is, what the
// Agents call it, and the two acts that change the account itself.

import type { ApiClient, ConnectionDto } from '../../api/client'
import { Button } from '../../primitives'
import { errorMessage, useAuthorizeConnection, useDeleteConnection } from '../../queries'
import { ConnectionStatus } from '../ConnectionStatus'
import { openAuthorization } from './openAuthorization'

import './connection.css'

export function ConnectionHeader({
  api,
  connection,
  onBack,
}: {
  api: ApiClient
  connection: ConnectionDto
  onBack: () => void
}) {
  const authorize = useAuthorizeConnection(api)
  const remove = useDeleteConnection(api)
  const title =
    connection.account == null
      ? connection.display_name
      : `${connection.display_name} · ${connection.account}`

  return (
    <div className="connection-head">
      <Button
        variant="ghost"
        aria-label="Back to Connections"
        className="connection-head-back"
        onClick={onBack}
      >
        ‹ Connections
      </Button>
      <span className="connection-head-monogram" aria-hidden="true">
        {connection.display_name.slice(0, 1).toUpperCase()}
      </span>
      <div className="connection-head-identity">
        <h1>{title}</h1>
        <span className="connection-hint">
          Sprites know it as <code>{connection.alias}</code>
        </span>
      </div>
      <ConnectionStatus status={connection.status} />
      <span className="connection-head-actions">
        <Button
          disabled={authorize.isPending}
          onClick={() =>
            authorize.mutate(
              {
                connectionId: connection.id,
                capabilities: connection.authorized_capabilities,
              },
              { onSuccess: (answer) => openAuthorization(answer.authorization_url) },
            )
          }
        >
          {authorize.isPending ? 'Waiting for the provider…' : 'Reconnect'}
        </Button>
        <Button
          variant="danger-quiet"
          disabled={remove.isPending}
          onClick={() => remove.mutate(connection.id, { onSuccess: onBack })}
        >
          Disconnect
        </Button>
      </span>
      {authorize.isError && (
        <p className="connection-error" role="alert">
          {errorMessage(authorize.error, 'The provider did not complete this.')}
        </p>
      )}
      {remove.isError && (
        <p className="connection-error" role="alert">
          {errorMessage(remove.error, 'That connection could not be disconnected.')}
        </p>
      )}
    </div>
  )
}
