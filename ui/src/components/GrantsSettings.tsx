// One Agent's host or Vault grant rules (ADR-0022).

import { useState } from 'react'
import { X } from 'lucide-react'

import type { ApiClient, GrantDto } from '../api/client'
import { Button, IconButton, Input } from '../primitives'
import { useRevokeGrant, useSetGrantRules } from '../queries'

import './GrantsSettings.css'

const RESOURCE_LABEL: Record<string, string> = {
  host: 'Host access',
  credential: 'Vault domains',
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
  credential: {
    empty: 'No allowed domains — every saved login asks for an approval.',
    placeholder: 'Add a domain, e.g. example.com',
    add: 'Add domain',
  },
}

export function GrantRow({
  api,
  grant,
  resourceLabel,
}: {
  api: ApiClient
  grant: GrantDto
  /** What the grant reaches, when the caller can name it: the machine of
   *  a host grant. The kind alone is the default. */
  resourceLabel?: string
}) {
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
          disabled={revoke.isPending}
          onClick={() => revoke.mutate(grant.id)}
        >
          Revoke
        </Button>
      </div>
      <div className="grant-rules">
        {grant.allow.length === 0 && (
          <span className="grant-rules-empty">{copy.empty}</span>
        )}
        {grant.allow.map((rule) => (
          <span key={rule} className="grant-rule">
            <code>{rule}</code>
            <IconButton
              icon={X}
              variant="ghost"
              size="sm"
              label={`Remove rule ${rule}`}
              disabled={setRules.isPending}
              onClick={() => removeRule(rule)}
            />
          </span>
        ))}
      </div>
      <div className="grant-rule-add">
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
          disabled={setRules.isPending || draft.trim() === ''}
          onClick={addRule}
        >
          {copy.add}
        </Button>
      </div>
    </div>
  )
}
