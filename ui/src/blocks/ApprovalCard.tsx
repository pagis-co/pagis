// The approval card: the block
// carries request_id plus denormalized title/body; the state always
// comes from the Request row, fetched here and invalidated by the
// `request.decided` and `request.superseded` WS events. Every card of
// one Request re-renders from the same query. The row's payload carries
// the daemon-derived rule proposal behind "Always allow" — a command
// class for a host action, the Credential record's own domain for a
// vault action, which is why a vault card can name the site it will
// open. A host command's rule covers every flag and argument of its
// command, and the card says so beside "Always allow" (ADR-0015). A
// Coding Session start proposes a session allow rule, and the payload
// carries the words of its checkbox. A Harness Permission names the
// harness, the machine and the directory, and its `execute` is a host
// command. A permission that the sprite gave to the Person shows the
// sprite's note under that line (ADR-0033).
//
// One card, one decision: the header names the act and the place,
// Approve is the only primary action, "Always allow" is a checkbox that
// widens that same Approve, and Deny is a ghost. A decided card keeps
// no controls: it collapses to one line with the decision and the time.

import { useState } from 'react'
import { KeyRound, Terminal } from 'lucide-react'

import type { ApiClient } from '../api/client'
import { Button } from '../primitives'
import { useAgentNames, useRequest, useDecideRequest } from '../queries'
import { Card, CardBody, CardFooter, CardHeader, SettledLine } from './Card'
import type { SettledTone } from './Card'

const STATE_LABEL: Record<string, string> = {
  approved: 'Approved',
  denied: 'Denied',
  expired: 'Expired',
  // The user sent a message in place of a decision.
  superseded: 'You replied instead',
}

const STATE_TONE: Record<string, SettledTone> = {
  approved: 'working',
  denied: 'failed',
}

export interface ApprovalPayload {
  proposed_rules?: string[]
  tool_name?: string
  domain?: string
  username?: string
  login_url?: string
  plugin_id?: string | null
  /** The words of the checkbox, when the daemon writes them. */
  always_label?: string | null
  /** A Harness Permission: the session, its machine and its directory. */
  session_id?: string
  harness_id?: string
  harness_name?: string
  host_id?: string
  host_name?: string
  directory?: string
  tool_call_id?: string
  /** The ACP tool kind, such as `execute` or `edit`. */
  tool_kind?: string
  command?: string | null
  locations?: string[]
  /** What the sprite asks the Person, when it gave the permission to
   *  the Person. */
  note?: string | null
}

/** The words of "Always allow": the daemon's own label, or a lead and
 *  the rules it writes. `null` when the request proposes no rule. The
 *  desktop card and the phone approval sheet both read these words. */
export function alwaysAllowWords(
  payload: ApprovalPayload | undefined,
): { lead: string; rules: string | null } | null {
  const label = payload?.always_label ?? null
  if (label !== null) return { lead: label, rules: null }
  const rules = payload?.proposed_rules ?? []
  if (rules.length === 0) return null
  return {
    lead: typeof payload?.plugin_id === 'string' ? 'Always allow this tool' : 'Always allow',
    rules: rules.join(', '),
  }
}

/** The reach of a host command rule, which "Always allow" states
 *  (ADR-0015). `null` when the rules are not host commands. */
export function alwaysAllowReach(
  kind: string | undefined,
  payload: ApprovalPayload | undefined,
): string | null {
  const rules = payload?.proposed_rules ?? []
  const hostCommand =
    payload?.tool_name === 'host_shell' ||
    (kind === 'harness_permission' && payload?.tool_kind === 'execute')
  if (!hostCommand || rules.length === 0) return null
  return rules.length === 1
    ? 'This rule also allows every flag and argument of the command, including flags that write files or run other programs.'
    : 'These rules also allow every flag and argument of their commands, including flags that write files or run other programs.'
}

/** The clock a settled card shows beside the decision. */
export function decidedClock(at: number | null | undefined): string | null {
  if (at == null) return null
  return new Date(at).toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit',
  })
}

export function ApprovalCard({
  api,
  requestId,
  title,
  body,
}: {
  api: ApiClient
  requestId: string
  title: string
  body: string
}) {
  const request = useRequest(api, requestId)
  const decide = useDecideRequest(api, requestId)
  const [always, setAlways] = useState(false)

  const state = request.data?.state
  const payload = request.data?.payload as ApprovalPayload | undefined
  const credential =
    request.data?.kind === 'credential_action' && payload?.domain !== undefined
  // A Plugin tool (ADR-0017): `Host` by default, so it waits
  // here, and its one rule is the tool's own qualified name.
  const pluginTool = typeof payload?.plugin_id === 'string'
  const harnessPermission = request.data?.kind === 'harness_permission'
  const alwaysWords = alwaysAllowWords(payload)
  const reach = alwaysAllowReach(request.data?.kind, payload)
  const alwaysId = `approval-always-${requestId}`
  const scopeId = `approval-always-scope-${requestId}`

  if (state !== undefined && state !== 'pending') {
    // Settled: one line, the decision and the time it was made.
    const clock = decidedClock(request.data?.decided_at)
    const label = STATE_LABEL[state] ?? state
    return (
      <Card className={`approval-${state}`} data-testid="approval-settled">
        <SettledLine
          tone={STATE_TONE[state] ?? 'neutral'}
          aside={clock !== null ? `${label} · ${clock}` : label}
        >
          {title}
        </SettledLine>
      </Card>
    )
  }

  return (
    <Card className="approval-card" data-testid="approval-card">
      <CardHeader
        icon={credential ? KeyRound : Terminal}
        act={title}
        place={
          credential ? (
            `${payload?.username} at ${payload?.domain}`
          ) : (
            <code>{body}</code>
          )
        }
      />
      <CardBody className="approval-card-where">
        {credential ? (
          <>
            Pagis opens <code>{payload?.login_url}</code> and types there.
          </>
        ) : pluginTool ? (
          "This tool is a plugin's. It runs on this computer with your privileges."
        ) : harnessPermission ? (
          <>
            {payload?.harness_name} runs this on {payload?.host_name} in{' '}
            <code>{payload?.directory}</code>.
          </>
        ) : (
          'Runs on this computer.'
        )}
      </CardBody>
      {harnessPermission && payload?.note != null && request.data !== undefined && (
        <AgentNote api={api} agentId={request.data.agent_id} note={payload.note} />
      )}
      {state === 'pending' && (
        <CardFooter>
          <Button
            variant="primary"
            className="approval-approve"
            disabled={decide.isPending}
            onClick={() =>
              decide.mutate(
                always
                  ? { decision: 'approved', scope: 'always' }
                  : { decision: 'approved' },
              )
            }
          >
            Approve
          </Button>
          {alwaysWords !== null && (
            <label className="approval-always" htmlFor={alwaysId}>
              <input
                id={alwaysId}
                type="checkbox"
                checked={always}
                disabled={decide.isPending}
                aria-describedby={reach !== null ? scopeId : undefined}
                onChange={(event) => setAlways(event.target.checked)}
              />
              <span>
                {alwaysWords.lead}
                {alwaysWords.rules !== null && (
                  <>
                    {' '}
                    <code>{alwaysWords.rules}</code>
                  </>
                )}
              </span>
            </label>
          )}
          {reach !== null && (
            <p id={scopeId} className="approval-always-scope">
              {reach}
            </p>
          )}
          <Button
            variant="ghost"
            className="approval-deny"
            disabled={decide.isPending}
            onClick={() => decide.mutate({ decision: 'denied' })}
          >
            Deny
          </Button>
        </CardFooter>
      )}
    </Card>
  )
}

/** The note of the sprite that gave a Harness Permission to the Person. */
function AgentNote({
  api,
  agentId,
  note,
}: {
  api: ApiClient
  agentId: string
  note: string
}) {
  const names = useAgentNames(api)
  const name = names[agentId] ?? 'Your sprite'
  return (
    <CardBody className="approval-card-note">
      {name} asks: {note}
    </CardBody>
  )
}
