// The Access section of a connection page: one row per live
// Agent, with the capabilities it holds as chips. Every change saves at
// once, so the section has no Save of its own.

import { Fragment, useState } from 'react'

import type { ApiClient, ConnectionDto, GrantDto } from '../../api/client'
import { Badge, Button, Frame, Row } from '../../primitives'
import {
  errorMessage,
  useCreateConnectionGrant,
  useRevokeGrant,
  useSetGrantCapabilities,
} from '../../queries'
import {
  CONNECTION_CAPABILITIES,
  CapabilityPicker,
} from '../ConnectionCapabilities'

import './connection.css'

type Agent = { id: string; name: string; status: string }

function capabilityLabel(name: string): string {
  return CONNECTION_CAPABILITIES.find((item) => item.name === name)?.label ?? name
}

export function ConnectionAccess({
  api,
  connection,
  agents,
  grants,
}: {
  api: ApiClient
  connection: ConnectionDto
  agents: Agent[]
  grants: GrantDto[]
}) {
  const create = useCreateConnectionGrant(api)
  const setCapabilities = useSetGrantCapabilities(api)
  const revoke = useRevokeGrant(api)
  const [editing, setEditing] = useState<string>()
  const live = agents.filter((agent) => agent.status !== 'archived')
  const grantOf = (agentId: string) =>
    grants.find(
      (grant) =>
        grant.agent_id === agentId &&
        grant.resource_kind === 'connection' &&
        grant.resource_id === connection.id,
    )
  const failure = create.error ?? setCapabilities.error ?? revoke.error

  return (
    <Frame>
      {live.map((agent) => {
        const grant = grantOf(agent.id)
        return (
          <Fragment key={agent.id}>
            <Row>
              <span className="connection-access-agent">{agent.name}</span>
              {grant === undefined ? (
                <span className="connection-hint">no access</span>
              ) : (
                grant.capabilities.map((capability) => (
                  <Badge key={capability} tone="accent">
                    {capabilityLabel(capability)}
                  </Badge>
                ))
              )}
              <span className="connection-row-spacer" />
              {grant === undefined ? (
                <Button
                  variant="ghost"
                  aria-label={`Grant to ${agent.name}`}
                  disabled={
                    create.isPending ||
                    connection.status !== 'connected' ||
                    connection.authorized_capabilities.length === 0
                  }
                  onClick={() =>
                    create.mutate({
                      agent_id: agent.id,
                      connection_id: connection.id,
                      capabilities: connection.authorized_capabilities,
                    })
                  }
                >
                  Grant
                </Button>
              ) : (
                <>
                  <Button
                    variant="ghost"
                    aria-label={`Change access for ${agent.name}`}
                    onClick={() =>
                      setEditing((open) => (open === agent.id ? undefined : agent.id))
                    }
                  >
                    Change
                  </Button>
                  <Button
                    variant="ghost"
                    aria-label={`Remove access for ${agent.name}`}
                    disabled={revoke.isPending}
                    onClick={() => revoke.mutate(grant.id)}
                  >
                    Remove
                  </Button>
                </>
              )}
            </Row>
            {grant !== undefined && editing === agent.id && (
              <Row>
                <CapabilityPicker
                  selected={grant.capabilities}
                  onChange={(capabilities) =>
                    setCapabilities.mutate({ grantId: grant.id, capabilities })
                  }
                />
              </Row>
            )}
          </Fragment>
        )
      })}
      {failure != null && (
        <Row>
          <p className="connection-error" role="alert">
            {errorMessage(failure, 'That access could not be changed.')}
          </p>
        </Row>
      )}
    </Frame>
  )
}
