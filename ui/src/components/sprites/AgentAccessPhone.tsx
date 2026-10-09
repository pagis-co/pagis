import { useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { Monitor } from 'lucide-react'
import type { AgentDto, ApiClient } from '../../api/client'
import { Badge, Button, Frame, Row, SectionLabel, Switch } from '../../primitives'
import {
  errorMessage,
  useConnections,
  useCreateConnectionGrant,
  useGrants,
  useHosts,
  useRevokeGrant,
  useSetGrantCapabilities,
  useSoftware,
} from '../../queries'
import { CONNECTION_CAPABILITIES } from '../ConnectionCapabilities'
import { NavBar } from '../phone/TopBar'

function capabilityLabels(names: string[]) {
  return names
    .map((name) => CONNECTION_CAPABILITIES.find((item) => item.name === name)?.label ?? name)
    .join(', ')
}

export function AgentAccessPhone({
  api,
  agent,
  resourceId,
}: {
  api: ApiClient
  agent: AgentDto
  resourceId?: string
}) {
  const navigate = useNavigate()
  const connections = useConnections(api)
  const grants = useGrants(api)
  const hosts = useHosts(api)
  const software = useSoftware(api)
  const create = useCreateConnectionGrant(api)
  const save = useSetGrantCapabilities(api)
  const revoke = useRevokeGrant(api)
  const [selected, setSelected] = useState<string[] | null>(null)
  const own = (grants.data ?? []).filter((row) => row.agent_id === agent.id)
  const connection = connections.data?.find((row) => row.id === resourceId)
  const grant = own.find((row) => row.resource_id === resourceId)
  const capabilities = selected ?? grant?.capabilities ?? []
  const busy = create.isPending || save.isPending || revoke.isPending
  const failure = create.error ?? save.error ?? revoke.error
  const back = () =>
    void navigate({
      to: resourceId ? '/sprites/$agentId/access' : '/sprites/$agentId',
      params: { agentId: agent.id },
    })
  const open = (id: string) =>
    void navigate({
      to: '/sprites/$agentId/access/$resourceId',
      params: { agentId: agent.id, resourceId: id },
    })
  const badge = (status: string | null | undefined, granted: boolean) =>
    status === 'reauth_required' ? (
      <Badge tone="waiting">Needs you</Badge>
    ) : granted ? (
      <Badge tone="working">Granted</Badge>
    ) : null
  return (
    <>
      <NavBar back={{ label: resourceId ? 'Access' : agent.name, onBack: back }} />
      <div className="phone-content">
        {resourceId ? (
          connection ? (
            <>
              <header className="phone-resource-head">
                <span className="phone-initial-tile phone-initial-tile-large">G</span>
                <div className="phone-row-copy">
                  <h1 className="phone-heading">
                    {connection.provider === 'google' ? 'Google' : connection.display_name}
                  </h1>
                  <p className="phone-hint">{connection.account ?? connection.alias}</p>
                </div>
                {badge(connection.status, !!grant)}
              </header>
              <section className="phone-section">
                <SectionLabel>What {agent.name} may do</SectionLabel>
                <Frame>
                  {CONNECTION_CAPABILITIES.map((item) => (
                    <Switch
                      key={item.name}
                      row
                      checked={capabilities.includes(item.name)}
                      disabled={busy}
                      onCheckedChange={(on) =>
                        setSelected(
                          on
                            ? [...capabilities, item.name]
                            : capabilities.filter((name) => name !== item.name),
                        )
                      }
                    >
                      {item.label}
                    </Switch>
                  ))}
                </Frame>
              </section>
              {capabilities.some((name) => !connection.authorized_capabilities.includes(name)) && (
                <p className="settings-warning">
                  This account has not authorized{' '}
                  {capabilityLabels(
                    capabilities.filter(
                      (name) => !connection.authorized_capabilities.includes(name),
                    ),
                  )}
                  . Save this access, then sign in again in Settings.
                </p>
              )}
              <Button
                variant="primary"
                size="lg"
                disabled={
                  busy || capabilities.length === 0 || (!grant && connection.status !== 'connected')
                }
                onClick={() =>
                  grant
                    ? save.mutate({ grantId: grant.id, capabilities })
                    : create.mutate({
                        agent_id: agent.id,
                        connection_id: connection.id,
                        capabilities,
                      })
                }
              >
                Save access
              </Button>
              {grant && (
                <Button
                  variant="link"
                  className="phone-danger"
                  disabled={busy}
                  onClick={() => revoke.mutate(grant.id, { onSuccess: back })}
                >
                  Revoke access
                </Button>
              )}
            </>
          ) : grant ? (
            <>
              <h1 className="phone-heading">
                {hosts.data?.find((host) => host.id === grant.resource_id)?.name ?? 'Host access'}
              </h1>
              <section className="phone-section">
                <SectionLabel>Host Allow Rules</SectionLabel>
                <Frame>
                  {grant.allow.map((rule) => (
                    <Row key={rule}>
                      <code>{rule}</code>
                    </Row>
                  ))}
                  {grant.sessions.map((rule) => (
                    <Row key={`${rule.harness}:${rule.directory}`}>
                      <span>
                        {rule.harness} sessions in {rule.directory}
                      </span>
                    </Row>
                  ))}
                  {grant.allow.length === 0 && grant.sessions.length === 0 && (
                    <Row>Every command asks for approval.</Row>
                  )}
                </Frame>
                <p className="phone-hint">Edit allow rules on a computer.</p>
              </section>
              <Button
                variant="link"
                className="phone-danger"
                disabled={busy}
                onClick={() => revoke.mutate(grant.id, { onSuccess: back })}
              >
                Revoke access
              </Button>
            </>
          ) : (
            <p
              role={connections.isError || grants.isError ? 'alert' : 'status'}
              className="phone-hint"
            >
              {connections.isPending || grants.isPending
                ? 'Reading access…'
                : 'That access is not on record.'}
            </p>
          )
        ) : (
          <>
            <div>
              <h1 className="phone-heading">Access</h1>
              <p className="phone-lead">
                {agent.name} reaches a Connection only where you grant it. A grant names the
                capabilities, and you can revoke it at any time.
              </p>
            </div>
            <section className="phone-section">
              <SectionLabel>Connections</SectionLabel>
              <Frame>
                {(connections.data ?? []).map((row) => {
                  const access = own.find(
                    (item) => item.resource_kind === 'connection' && item.resource_id === row.id,
                  )
                  return (
                    <Row key={row.id} chevron onClick={() => open(row.id)}>
                      <span className="phone-initial-tile">G</span>
                      <span className="phone-row-copy">
                        <span className="phone-row-line phone-row-account">
                          {row.account ?? row.alias}
                        </span>
                        <span className="phone-row-line">
                          {access ? capabilityLabels(access.capabilities) : 'No access'}
                          {row.status === 'reauth_required' ? ' · Sign in again in Settings' : ''}
                        </span>
                      </span>
                      {badge(row.status, !!access)}
                    </Row>
                  )
                })}
              </Frame>
            </section>
            <section className="phone-section">
              <SectionLabel>Computers</SectionLabel>
              <Frame>
                {own
                  .filter((row) => row.resource_kind === 'host')
                  .map((row) => (
                    <Row
                      key={row.id}
                      chevron
                      onClick={() => {
                        if (row.resource_id) open(row.resource_id)
                      }}
                    >
                      <span className="phone-initial-tile">
                        <Monitor size={20} aria-hidden />
                      </span>
                      <span className="phone-row-copy">
                        <span>
                          {hosts.data?.find((host) => host.id === row.resource_id)?.name ??
                            'Computer'}
                        </span>
                        <span className="phone-hint">Host access</span>
                      </span>
                      <Badge tone="working">Granted</Badge>
                    </Row>
                  ))}
              </Frame>
            </section>
            <div className="phone-section">
              <p className="phone-hint">
                Every sprite can use the {software.data?.length ?? 0} packages in Software.
              </p>
              <p className="phone-hint">Connect another account in Settings › Connections.</p>
            </div>
          </>
        )}
        {(connections.isError || grants.isError || failure) && (
          <p role="alert" className="phone-hint">
            {failure
              ? errorMessage(failure, 'That access could not be saved.')
              : 'Could not read access.'}
          </p>
        )}
      </div>
    </>
  )
}
