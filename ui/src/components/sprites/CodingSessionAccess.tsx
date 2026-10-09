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
//
// This file holds the state, the words and the desktop layout. The
// phone layout (`CodingSessionAccessPhone`) uses the same state and
// words.

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

export const CODING_SESSIONS_TITLE = 'Coding sessions'
export const UNATTENDED_LABEL = 'Allow modes that act without asking'
export const NO_MACHINES =
  'No computer of yours can start a coding harness. Open the Pagis client on a computer that has one.'

/** The words of the modes, from ADR-0033. */
export const MODES: { value: SessionApprovalMode; label: string }[] = [
  { value: 'person', label: 'Ask me' },
  { value: 'agent', label: 'Let the sprite decide' },
]

export const sectionLead = (agent: AgentDto) =>
  `The widest approval mode ${agent.name} may use for a coding session on each computer.`

export const modeLabel = (host: HostDto) => `Approvals for coding sessions on ${host.name}`

/** What `mode` does, in one line. */
export function modeEffect(mode: SessionApprovalMode, agent: AgentDto): string {
  switch (mode) {
    case 'person':
      return 'You answer each permission that Pagis does not allow by its own rules.'
    case 'agent':
      return `${agent.name} answers each permission that Pagis does not allow by its own rules, or asks you.`
  }
}

export const unattendedHint = (agent: AgentDto) =>
  `${agent.name} may run a coding harness in a mode that does not ask first, such as ` +
  'Bypass permissions of Claude Code, and a harness that never asks, such as pi.'

export const unattendedWarning = (host: HostDto) =>
  `A coding harness can then run any command as you on ${host.name}, with no question.`

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

/** The computers of the Person that can start a Coding Harness, each
 *  with the host Grant of `agent` there. `machines` is undefined until
 *  the computers and the Grants are read, so a row never shows a
 *  default in place of the saved values. `none` is true when no
 *  computer can start a harness. */
export function useCodingMachines(api: ApiClient, agent: AgentDto) {
  const hosts = useHosts(api)
  const grants = useGrants(api)
  const capable = (hosts.data ?? []).filter(startsHarness)
  const machines =
    grants.data === undefined
      ? undefined
      : capable.map((host) => ({ host, grant: hostGrant(agent, host, grants.data) }))
  return { machines, none: hosts.data !== undefined && capable.length === 0 }
}

/** The widest mode of `agent` on `host`, and its change. The choice
 *  shows while it saves. After a failure the saved mode shows again. */
export function useMachineMode(
  api: ApiClient,
  agent: AgentDto,
  host: HostDto,
  grant: GrantDto | undefined,
) {
  const setMode = useSetSessionApprovalMode(api)
  const mode = setMode.isPending
    ? setMode.variables.mode
    : (grant?.session_approval_mode ?? 'person')
  return {
    mode,
    pending: setMode.isPending,
    error: setMode.isError
      ? errorMessage(setMode.error, 'The approval mode did not change.')
      : undefined,
    change: (next: string) =>
      setMode.mutate({ agentId: agent.id, hostId: host.id, mode: next as SessionApprovalMode }),
  }
}

/** Whether `agent` may use Unattended Modes on `host`, and its change.
 *  The choice shows while it saves. After a failure the saved value
 *  shows again. */
export function useMachineUnattended(
  api: ApiClient,
  agent: AgentDto,
  host: HostDto,
  grant: GrantDto | undefined,
) {
  const setAllowed = useSetUnattendedModes(api)
  const allowed = setAllowed.isPending
    ? setAllowed.variables.allowed
    : (grant?.unattended_modes ?? false)
  return {
    allowed,
    pending: setAllowed.isPending,
    error: setAllowed.isError
      ? errorMessage(setAllowed.error, 'The allowance of modes did not change.')
      : undefined,
    change: (next: boolean) =>
      setAllowed.mutate({ agentId: agent.id, hostId: host.id, allowed: next }),
  }
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
  const mode = useMachineMode(api, agent, host, grant)
  const unattended = useMachineUnattended(api, agent, host, grant)

  return (
    <div className="coding-session-access-row" data-testid="coding-session-access-row">
      <strong>{host.name}</strong>
      <Select
        label={modeLabel(host)}
        value={mode.mode}
        disabled={mode.pending}
        items={MODES}
        onValueChange={mode.change}
      />
      <p className="settings-hint">{modeEffect(mode.mode, agent)}</p>
      {mode.error && (
        <p role="alert" className="settings-error">
          {mode.error}
        </p>
      )}
      <Switch
        checked={unattended.allowed}
        disabled={unattended.pending}
        onCheckedChange={unattended.change}
      >
        {UNATTENDED_LABEL}
      </Switch>
      <p className="settings-hint">{unattendedHint(agent)}</p>
      {unattended.allowed && <p className="settings-warning">{unattendedWarning(host)}</p>}
      {unattended.error && (
        <p role="alert" className="settings-error">
          {unattended.error}
        </p>
      )}
    </div>
  )
}

export function CodingSessionAccess({ api, agent }: { api: ApiClient; agent: AgentDto }) {
  const { machines, none } = useCodingMachines(api, agent)

  return (
    <section className="agent-access-connection" aria-labelledby="coding-session-access-title">
      <h3 id="coding-session-access-title">{CODING_SESSIONS_TITLE}</h3>
      <p className="settings-hint">{sectionLead(agent)}</p>
      {none && <p className="settings-hint">{NO_MACHINES}</p>}
      {machines?.map(({ host, grant }) => (
        <MachineMode key={host.id} api={api} agent={agent} host={host} grant={grant} />
      ))}
    </section>
  )
}
