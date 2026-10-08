// The Access section of an Agent profile: what this Agent may
// reach. One card per Connection with its named capabilities, the
// widest Session Approval Mode on each computer (ADR-0033), the host
// and Vault rules (ADR-0022), and the Software List that every Agent
// reads.

import { useState } from 'react'

import type { AgentDto, ApiClient } from '../../api/client'
import { Button } from '../../primitives'
import {
  useConnections,
  useCreateConnectionGrant,
  useGrants,
  useHosts,
  useRevokeGrant,
  useSetGrantCapabilities,
  useSoftware,
} from '../../queries'
import {
  CapabilityPicker,
  CONNECTION_CAPABILITIES,
} from '../ConnectionCapabilities'
import { GrantRow } from '../GrantsSettings'
import { CodingSessionAccess } from './CodingSessionAccess'
import type { GrantDto, HostDto } from '../../api/client'

import '../agent.css'
import '../settings.css'

/** The Software List. Every Agent reads the whole list, and a
 *  package is loaded by `tool_search`, so there is no grant to edit. */
function SoftwareAccess({ api }: { api: ApiClient }) {
  const packages = useSoftware(api)
  const count = (packages.data ?? []).length
  return (
    <p className="settings-hint">
      Software List: {count} package{count === 1 ? '' : 's'}, available to every
      sprite, loaded on search.
    </p>
  )
}

/** What a host grant reaches: the machine it names. A machine this
 *  person no longer has is named by nothing, so the kind stands. */
function hostLabel(grant: GrantDto, hosts: HostDto[]): string | undefined {
  if (grant.resource_kind !== 'host') return undefined
  const host = hosts.find((entry) => entry.id === grant.resource_id)
  return host === undefined ? undefined : `Host access: ${host.name}`
}

export function AgentAccess({ api, agent }: { api: ApiClient; agent: AgentDto }) {
  const connections = useConnections(api)
  const grants = useGrants(api)
  const hosts = useHosts(api)
  const createGrant = useCreateConnectionGrant(api)
  const setCapabilities = useSetGrantCapabilities(api)
  const revokeGrant = useRevokeGrant(api)
  const [selected, setSelected] = useState<Record<string, string[]>>({})
  // The hint that no Google account is connected stands here, where
  // the grant it feeds is made.
  const noGoogle =
    connections.data !== undefined &&
    !connections.data.some((connection) => connection.provider === 'google')

  return (
    <section className="agent-access" aria-label={`${agent.name} access details`}>
      <p className="settings-hint">
        {agent.name} reaches a Connection only where you grant it. A grant
        names the capabilities, and you can revoke it at any time.
      </p>
      <SoftwareAccess api={api} />
      {noGoogle && (
        <p className="settings-hint" data-testid="agent-access-no-google" role="status">
          No Google account is connected. Connect one in Settings → Connections
          and {agent.name} can read Gmail and Calendar for you.
        </p>
      )}
      {(connections.data ?? []).map((connection) => {
        const grant = (grants.data ?? []).find(
          (item) =>
            item.agent_id === agent.id &&
            item.resource_kind === 'connection' &&
            item.resource_id === connection.id,
        )
        const capabilities =
          selected[connection.id] ?? grant?.capabilities ?? []
        const missingAuthorization = capabilities.filter(
          (capability) => !connection.authorized_capabilities.includes(capability),
        )
        const missingLabels = missingAuthorization.map(
          (capability) =>
            CONNECTION_CAPABILITIES.find((item) => item.name === capability)?.label ??
            capability,
        )
        return (
          <div className="agent-access-connection" key={connection.id}>
            <strong>{connection.display_name}</strong>
            <span>{connection.account ?? connection.alias}</span>
            <CapabilityPicker
              selected={capabilities}
              onChange={(next) =>
                setSelected((current) => ({ ...current, [connection.id]: next }))
              }
            />
            {missingLabels.length > 0 && (
              <p className="settings-warning">
                {connection.display_name} has not authorized {missingLabels.join(', ')}.
                {' Save this access, then reauthorize the account in Connections.'}
              </p>
            )}
            <Button
              variant="primary"
              aria-label={
                grant === undefined
                  ? `Grant ${connection.display_name} to ${agent.name}`
                  : `Save ${connection.display_name} access`
              }
              disabled={
                createGrant.isPending ||
                setCapabilities.isPending ||
                (grant === undefined && connection.status !== 'connected') ||
                capabilities.length === 0
              }
              onClick={() => {
                if (grant === undefined) {
                  createGrant.mutate({
                    agent_id: agent.id,
                    connection_id: connection.id,
                    capabilities,
                  })
                } else {
                  setCapabilities.mutate({ grantId: grant.id, capabilities })
                }
              }}
            >
              {grant === undefined ? 'Grant access' : 'Save access'}
            </Button>
            {grant !== undefined && (
              <Button
                variant="danger"
                aria-label={`Revoke ${connection.display_name} access`}
                disabled={revokeGrant.isPending}
                onClick={() => revokeGrant.mutate(grant.id)}
              >
                Revoke access
              </Button>
            )}
          </div>
        )
      })}
      <CodingSessionAccess api={api} agent={agent} />
      {(grants.data ?? [])
        .filter(
          (grant) =>
            grant.agent_id === agent.id && grant.resource_kind !== 'connection',
        )
        .map((grant) => (
          <GrantRow
            key={grant.id}
            api={api}
            grant={grant}
            resourceLabel={hostLabel(grant, hosts.data ?? [])}
          />
        ))}
    </section>
  )
}
