// The Coding sessions section of the Access tab (ADR-0033): the widest
// Session Approval Mode of an Agent on each computer of the Person that
// can start a Coding Harness. The host Grant of the Agent on the
// computer holds the mode. With no Grant the mode is `person`, and the
// first wider choice makes the Grant. A choice saves at once, and the
// change is a Grant revision.

import type { AgentDto, ApiClient, GrantDto, HostDto, SessionApprovalMode } from '../../api/client'
import { Select } from '../../primitives'
import { errorMessage, useGrants, useHosts, useSetSessionApprovalMode } from '../../queries'

import '../agent.css'
import '../settings.css'

const TITLE = 'Coding sessions'

/** The words of the modes, from ADR-0033. */
const MODES: { value: SessionApprovalMode; label: string }[] = [
  { value: 'person', label: 'Ask me' },
  { value: 'agent', label: 'Let the sprite decide' },
]

/** A computer that declared at least one Coding Harness it can start. */
function startsHarness(host: HostDto): boolean {
  return host.capabilities.some((capability) => capability.startsWith('harness:'))
}

/** The widest mode of `agent` on `host`: the mode on its host Grant
 *  there, else `person`. */
function savedMode(agent: AgentDto, host: HostDto, grants: GrantDto[]): SessionApprovalMode {
  const grant = grants.find(
    (item) =>
      item.agent_id === agent.id &&
      item.resource_kind === 'host' &&
      item.resource_id === host.id,
  )
  return grant?.session_approval_mode ?? 'person'
}

/** What `mode` does, in one line. */
function ModeEffect({ mode, agent }: { mode: SessionApprovalMode; agent: AgentDto }) {
  switch (mode) {
    case 'person':
      return (
        <p className="settings-hint">
          You answer each permission that Pagis does not allow by its own rules.
        </p>
      )
    case 'agent':
      return (
        <p className="settings-hint">
          {`${agent.name} answers each permission that Pagis does not allow by its own rules, or asks you.`}
        </p>
      )
  }
}

function MachineMode({
  api,
  agent,
  host,
  saved,
}: {
  api: ApiClient
  agent: AgentDto
  host: HostDto
  saved: SessionApprovalMode
}) {
  const setMode = useSetSessionApprovalMode(api)
  // The choice shows while it saves. After a failure the saved mode
  // shows again.
  const mode = setMode.isPending ? setMode.variables.mode : saved

  return (
    <div className="coding-session-access-row" data-testid="coding-session-access-row">
      <strong>{host.name}</strong>
      <Select
        label={`Approvals for coding sessions on ${host.name}`}
        value={mode}
        disabled={setMode.isPending}
        items={MODES}
        onValueChange={(value) =>
          setMode.mutate({
            agentId: agent.id,
            hostId: host.id,
            mode: value as SessionApprovalMode,
          })
        }
      />
      <ModeEffect mode={mode} agent={agent} />
      {setMode.isError && (
        <p role="alert" className="settings-error">
          {errorMessage(setMode.error, 'The approval mode did not change.')}
        </p>
      )}
    </div>
  )
}

export function CodingSessionAccess({ api, agent }: { api: ApiClient; agent: AgentDto }) {
  const hosts = useHosts(api)
  const grants = useGrants(api)
  const machines = (hosts.data ?? []).filter(startsHarness)
  // A row waits for the Grants, so it never shows a default in place of
  // the saved mode.
  const saved = grants.data

  return (
    <section className="agent-access-connection" aria-labelledby="coding-session-access-title">
      <h3 id="coding-session-access-title">{TITLE}</h3>
      <p className="settings-hint">
        {`The widest approval mode ${agent.name} may use for a coding session on each computer.`}
      </p>
      {hosts.data !== undefined && machines.length === 0 && (
        <p className="settings-hint">
          No computer of yours can start a coding harness. Open the Pagis client on a computer
          that has one.
        </p>
      )}
      {saved !== undefined &&
        machines.map((host) => (
          <MachineMode
            key={host.id}
            api={api}
            agent={agent}
            host={host}
            saved={savedMode(agent, host, saved)}
          />
        ))}
    </section>
  )
}
