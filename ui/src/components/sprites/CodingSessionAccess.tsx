// The Coding sessions section of the Access tab (ADR-0033): the widest
// Session Approval Mode of an Agent on each computer of the Person that
// can start a Coding Harness, and whether the Agent may use Unattended
// Modes there. The host Grant of the Agent on the computer holds both.
// With no Grant the mode is `person` and Unattended Modes are not
// allowed, and the first change makes the Grant. A change saves at
// once, and it is a Grant revision.
//
// The allowance is a switch apart from the mode: the mode says who
// answers a Harness Permission, and the switch says whether the harness
// may act with no question.

import type { AgentDto, ApiClient, GrantDto, HostDto, SessionApprovalMode } from '../../api/client'
import { Select, Switch } from '../../primitives'
import {
  errorMessage,
  useGrants,
  useHosts,
  useSetSessionApprovalMode,
  useSetUnattendedModes,
} from '../../queries'

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

/** The host Grant of `agent` on `host`, if there is one. */
function hostGrant(agent: AgentDto, host: HostDto, grants: GrantDto[]): GrantDto | undefined {
  return grants.find(
    (item) =>
      item.agent_id === agent.id &&
      item.resource_kind === 'host' &&
      item.resource_id === host.id,
  )
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

/** Whether `agent` may use Unattended Modes on `host`. */
function UnattendedModes({
  api,
  agent,
  host,
  saved,
}: {
  api: ApiClient
  agent: AgentDto
  host: HostDto
  saved: boolean
}) {
  const setAllowed = useSetUnattendedModes(api)
  // The choice shows while it saves. After a failure the saved value
  // shows again.
  const allowed = setAllowed.isPending ? setAllowed.variables.allowed : saved

  return (
    <>
      <Switch
        checked={allowed}
        disabled={setAllowed.isPending}
        onCheckedChange={(next) =>
          setAllowed.mutate({ agentId: agent.id, hostId: host.id, allowed: next })
        }
      >
        Allow modes that act without asking
      </Switch>
      <p className="settings-hint">
        {`${agent.name} may run a coding harness in a mode that does not ask first, such as ` +
          'Bypass permissions of Claude Code, and a harness that never asks, such as pi.'}
      </p>
      {allowed && (
        <p className="settings-warning">
          {`A coding harness can then run any command as you on ${host.name}, with no question.`}
        </p>
      )}
      {setAllowed.isError && (
        <p role="alert" className="settings-error">
          {errorMessage(setAllowed.error, 'The allowance of modes did not change.')}
        </p>
      )}
    </>
  )
}

function MachineMode({
  api,
  agent,
  host,
  grant,
}: {
  api: ApiClient
  agent: AgentDto
  host: HostDto
  grant: GrantDto | undefined
}) {
  const setMode = useSetSessionApprovalMode(api)
  // The choice shows while it saves. After a failure the saved mode
  // shows again.
  const mode = setMode.isPending
    ? setMode.variables.mode
    : (grant?.session_approval_mode ?? 'person')

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
      <UnattendedModes
        api={api}
        agent={agent}
        host={host}
        saved={grant?.unattended_modes ?? false}
      />
    </div>
  )
}

export function CodingSessionAccess({ api, agent }: { api: ApiClient; agent: AgentDto }) {
  const hosts = useHosts(api)
  const grants = useGrants(api)
  const machines = (hosts.data ?? []).filter(startsHarness)
  // A row waits for the Grants, so it never shows a default in place of
  // the saved values.
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
            grant={hostGrant(agent, host, saved)}
          />
        ))}
    </section>
  )
}
