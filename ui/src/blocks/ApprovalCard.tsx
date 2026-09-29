// The approval card: the block
// carries request_id plus denormalized title/body; the state always
// comes from the Request row, fetched here and invalidated by the
// `request.decided` and `request.superseded` WS events. Every card of
// one Request re-renders from the same query. The row's payload carries
// the daemon-derived rule proposal behind "Always allow" — a command
// class for a host action, the Credential record's own domain for a
// vault action, which is why a vault card can name the site it will
// open. A host command's rule covers every flag and argument of its
// command, and the card says so beside "Always allow" (ADR-0015).
//
// One card, one decision: the header names the act and the place,
// Approve is the only primary action, "Always allow" is a checkbox that
// widens that same Approve, and Deny is a ghost. A decided card keeps
// no controls: it collapses to one line with the decision and the time.

import { useState } from 'react'
import { KeyRound, Terminal } from 'lucide-react'

import type { ApiClient } from '../api/client'
import { Button } from '../primitives'
import { useRequest, useDecideRequest } from '../queries'
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

interface ApprovalPayload {
  proposed_rules?: string[]
  tool_name?: string
  domain?: string
  username?: string
  login_url?: string
  plugin_id?: string | null
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
  const proposedRules = payload?.proposed_rules ?? []
  const credential =
    request.data?.kind === 'credential_action' && payload?.domain !== undefined
  // A Plugin tool (ADR-0017): `Host` by default, so it waits
  // here, and its one rule is the tool's own qualified name.
  const pluginTool = typeof payload?.plugin_id === 'string'
  const hostCommand = payload?.tool_name === 'host_shell'
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
        ) : (
          'Runs on this computer.'
        )}
      </CardBody>
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
          {proposedRules.length > 0 && (
            <label className="approval-always" htmlFor={alwaysId}>
              <input
                id={alwaysId}
                type="checkbox"
                checked={always}
                disabled={decide.isPending}
                aria-describedby={hostCommand ? scopeId : undefined}
                onChange={(event) => setAlways(event.target.checked)}
              />
              <span>
                {pluginTool ? 'Always allow this tool' : 'Always allow'}{' '}
                <code>{proposedRules.join(', ')}</code>
              </span>
            </label>
          )}
          {proposedRules.length > 0 && hostCommand && (
            <p id={scopeId} className="approval-always-scope">
              {proposedRules.length === 1
                ? 'This rule also allows every flag and argument of the command, including flags that write files or run other programs.'
                : 'These rules also allow every flag and argument of their commands, including flags that write files or run other programs.'}
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
