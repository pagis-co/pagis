// The Coding sessions section of the Access tab at phone width: a frame
// for each computer that can start a Coding Harness: its name, the widest
// Session Approval Mode and the switch of Unattended Modes. The state
// and the words come from `CodingSessionAccess`.

import { Monitor } from 'lucide-react'
import type { AgentDto, ApiClient, GrantDto, HostDto } from '../../api/client'
import { Frame, Row, SectionLabel, Select, Switch } from '../../primitives'
import {
  CODING_SESSIONS_TITLE,
  MODES,
  NO_MACHINES,
  UNATTENDED_LABEL,
  modeEffect,
  modeLabel,
  sectionLead,
  unattendedHint,
  unattendedWarning,
  useCodingMachines,
  useMachineMode,
  useMachineUnattended,
} from './CodingSessionAccess'

import '../settings.css'

function Machine({
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
    <>
      <Frame data-testid="coding-session-access-row">
        <Row>
          <span className="phone-initial-tile">
            <Monitor size={20} aria-hidden />
          </span>
          <span>{host.name}</span>
        </Row>
        <Row
          value={
            <Select
              label={modeLabel(host)}
              value={mode.mode}
              disabled={mode.pending}
              items={MODES}
              onValueChange={mode.change}
            />
          }
        >
          Approvals
        </Row>
        <Switch
          row
          checked={unattended.allowed}
          disabled={unattended.pending}
          onCheckedChange={unattended.change}
        >
          {UNATTENDED_LABEL}
        </Switch>
      </Frame>
      <p className="phone-hint">{modeEffect(mode.mode, agent)}</p>
      <p className="phone-hint">{unattendedHint(agent)}</p>
      {unattended.allowed && <p className="settings-warning">{unattendedWarning(host)}</p>}
      {[mode.error, unattended.error].filter(Boolean).map((error) => (
        <p key={error} role="alert" className="phone-hint">
          {error}
        </p>
      ))}
    </>
  )
}

export function CodingSessionAccessPhone({ api, agent }: { api: ApiClient; agent: AgentDto }) {
  const { machines, none } = useCodingMachines(api, agent)
  return (
    <section className="phone-section" aria-labelledby="coding-session-access-phone-title">
      <SectionLabel id="coding-session-access-phone-title">{CODING_SESSIONS_TITLE}</SectionLabel>
      <p className="phone-hint">{none ? NO_MACHINES : sectionLead(agent)}</p>
      {machines?.map(({ host, grant }) => (
        <Machine key={host.id} api={api} agent={agent} host={host} grant={grant} />
      ))}
    </section>
  )
}
