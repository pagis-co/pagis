// One Agent's host or Vault grant rules, and the session allow rules of a
// host grant (ADR-0022, ADR-0033).

import { useState } from 'react'
import { useWorkspaceAccess } from '../workspace'
import { X } from 'lucide-react'

import type { ApiClient, GrantDto, SessionAllowRule } from '../api/client'
import { Button, IconButton, Input } from '../primitives'
import { useColleagues, useRevokeGrant, useSetGrantRules, useWorkspaceMailbox, useCreateWorkspaceMailboxGrant, useCreateCalendarGrant, useGrants } from '../queries'

import './GrantsSettings.css'

const RESOURCE_LABEL: Record<string, string> = {
  host: 'Host access',
  colleagues: 'Messages to colleagues',
  credential: 'Vault domains',
  workspace_mailbox: 'Workspace Mailbox',
  calendar: 'Calendar',
}

// What one grant's rules are, per resource kind.
const RULE_COPY: Record<
  string,
  { empty: string; placeholder: string; add: string }
> = {
  host: {
    empty: 'No allow rules — every command asks for an approval.',
    placeholder: 'Add a command prefix, e.g. git status',
    add: 'Add rule',
  },
  workspace_mailbox: {
    empty: 'No allowed domains — every message needs an Approval.',
    placeholder: 'Add a domain, e.g. example.com',
    add: 'Add domain',
  },
  credential: {
    empty: 'No allowed domains — every saved login asks for an approval.',
    placeholder: 'Add a domain, e.g. example.com',
    add: 'Add domain',
  },
}

export function GrantRow(props: { api: ApiClient; grant: GrantDto; resourceLabel?: string }) {
  return props.grant.resource_kind === 'colleagues'
    ? <ColleagueGrantRow {...props} />
    : <GrantRulesRow {...props} />
}

function ColleagueGrantRow(props: { api: ApiClient; grant: GrantDto; resourceLabel?: string }) {
  const colleagues = useColleagues(props.api)
  const ruleNames = Object.fromEntries((colleagues.data ?? []).map((person) => [person.workspace_id, person.name]))
  return <GrantRulesRow {...props} ruleNames={ruleNames} />
}

function GrantRulesRow({
  api,
  grant,
  resourceLabel,
  ruleNames,
}: {
  api: ApiClient
  grant: GrantDto
  /** What the grant reaches, when the caller can name it: the machine of
   *  a host grant. The kind alone is the default. */
  resourceLabel?: string
  ruleNames?: Record<string, string>
}) {
  const { canManage } = useWorkspaceAccess()
  const setRules = useSetGrantRules(api)
  const revoke = useRevokeGrant(api)
  const [draft, setDraft] = useState('')
  const copy = RULE_COPY[grant.resource_kind] ?? RULE_COPY.host

  const addRule = () => {
    const rule = draft.trim()
    if (rule === '' || grant.allow.includes(rule)) return
    setRules.mutate(
      { grantId: grant.id, allow: [...grant.allow, rule] },
      { onSuccess: () => setDraft('') },
    )
  }

  const removeRule = (rule: string) => {
    setRules.mutate({
      grantId: grant.id,
      allow: grant.allow.filter((r) => r !== rule),
    })
  }

  const removeSessionRule = (rule: SessionAllowRule) => {
    setRules.mutate({
      grantId: grant.id,
      allow: grant.allow,
      sessions: grant.sessions.filter(
        (r) => r.harness !== rule.harness || r.directory !== rule.directory,
      ),
    })
  }

  return (
    <div className="grant-row" data-testid="grant-row">
      <div className="grant-row-header">
        <span className="grant-agent">{grant.agent_name}</span>
        <span className="grant-resource">
          {resourceLabel ?? RESOURCE_LABEL[grant.resource_kind] ?? grant.resource_kind}
        </span>
        <Button
          variant="danger"
          size="sm"
          className="grant-revoke"
          disabled={!canManage || revoke.isPending}
          onClick={() => revoke.mutate(grant.id)}
        >
          Revoke
        </Button>
      </div>
      {grant.is_default && <p className="settings-hint">Default for the Chief of Staff</p>}
      {grant.resource_kind === 'calendar' ? <p className="settings-hint">Read Free/Busy and Events, create and change Events, and answer invitations.</p> : <>
      <div className="grant-rules">
        {grant.allow.length === 0 && grant.sessions.length === 0 && (
          <span className="grant-rules-empty">{ruleNames ? 'Every message needs an Approval.' : copy.empty}</span>
        )}
        {grant.allow.map((rule) => (
          <span key={rule} className="grant-rule">
            <code>{ruleNames ? (ruleNames[rule] ?? 'Unavailable colleague') : rule}</code>
            <IconButton
              icon={X}
              variant="ghost"
              size="sm"
              label={`Remove rule ${ruleNames ? (ruleNames[rule] ?? 'Unavailable colleague') : rule}`}
              disabled={!canManage || setRules.isPending}
              onClick={() => removeRule(rule)}
            />
          </span>
        ))}
        {grant.sessions.map((rule) => {
          const text = `${rule.harness} sessions in ${rule.directory}`
          return (
            <span key={`${rule.harness} ${rule.directory}`} className="grant-rule">
              <span>{text}</span>
              <IconButton
                icon={X}
                variant="ghost"
                size="sm"
                label={`Remove rule ${text}`}
                disabled={!canManage || setRules.isPending}
                onClick={() => removeSessionRule(rule)}
              />
            </span>
          )
        })}
      </div>
      {ruleNames ? <p className="settings-hint">Approve a message with Always allow to add colleagues.</p> : <div className="grant-rule-add">
        <Input
          placeholder={copy.placeholder}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') addRule()
          }}
        />
        <Button
          variant="primary"
          disabled={!canManage || setRules.isPending || draft.trim() === ''}
          onClick={addRule}
        >
          {copy.add}
        </Button>
      </div>}
      </>}
    </div>
  )
}

export function WorkspaceMailboxAccess({ api, agent }: { api: ApiClient; agent: { id: string; name: string } }) {
  const { canManage } = useWorkspaceAccess()
  const mailbox = useWorkspaceMailbox(api)
  const grants = useGrants(api)
  const create = useCreateWorkspaceMailboxGrant(api)
  const own = (grants.data ?? []).filter((grant) => grant.agent_id === agent.id && grant.resource_kind === 'workspace_mailbox')
  if (!mailbox.data?.mailbox) return null
  return <div className="agent-access-connection">
    <strong>Workspace Mailbox: {mailbox.data.mailbox.address}</strong>
    {own.length === 0 && <Button
      aria-label={`Grant Workspace Mailbox to ${agent.name}`}
      disabled={!canManage || grants.isPending || create.isPending}
      onClick={() => create.mutate({ agent_id: agent.id })}
    >Grant access</Button>}
    {create.isError && <p role="alert">Workspace Mailbox access did not change.</p>}
  </div>
}


export function CalendarAccess({ api, agent }: { api: ApiClient; agent: { id: string; name: string } }) {
  const { canManage } = useWorkspaceAccess()
  const grants = useGrants(api)
  const create = useCreateCalendarGrant(api)
  const own = (grants.data ?? []).some(grant => grant.agent_id === agent.id && grant.resource_kind === 'calendar')
  return <div className="agent-access-connection">
    <strong>Calendar</strong>
    {!own && <Button
      aria-label={`Grant Calendar to ${agent.name}`}
      disabled={!canManage || grants.isPending || create.isPending}
      onClick={() => create.mutate({ agent_id: agent.id })}
    >Grant access</Button>}
    {create.isError && <p role="alert">Calendar access did not change.</p>}
  </div>
}
