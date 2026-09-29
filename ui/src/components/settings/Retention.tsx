import { useState } from 'react'

import type { ApiClient, RetentionPolicyDto } from '../../api/client'
import { Button, Frame, Input, Row } from '../../primitives'
import { errorMessage, useRetentionPolicies, useSetRetentionPolicy } from '../../queries'

import './Retention.css'

/** The name each artifact class shows under. */
const CLASS_NAMES: Record<string, string> = {
  screenshot: 'Screenshots',
  call_recording: 'Call recordings',
  call_transcript: 'Call transcripts',
  file: 'Files',
}

function className(kind: string): string {
  return CLASS_NAMES[kind] ?? kind
}

function PolicyRow({ api, policy }: { api: ApiClient; policy: RetentionPolicyDto }) {
  const set = useSetRetentionPolicy(api)
  const [days, setDays] = useState(
    policy.retain_days === null || policy.retain_days === undefined
      ? ''
      : String(policy.retain_days),
  )
  const name = className(policy.kind)
  const keepForEver = days.trim() === ''
  const parsed = Number(days)
  const valid = keepForEver || (Number.isInteger(parsed) && parsed >= 1)

  return (
    <Row className="settings-retention-row">
      <span className="settings-retention-name">{name}</span>
      <Input
        type="number"
        min={1}
        className="settings-retention-days"
        aria-label={`Days to keep ${name}`}
        placeholder="Keep for ever"
        value={days}
        onChange={(event) => setDays(event.target.value)}
      />
      <span className="settings-retention-unit">days</span>
      {set.isError && (
        <span className="settings-retention-error" role="alert">
          {errorMessage(set.error, 'That window could not be saved.')}
        </span>
      )}
      <Button
        size="sm"
        className="settings-retention-save"
        aria-label={`Save the window for ${name}`}
        disabled={!valid || set.isPending}
        onClick={() =>
          set.mutate({ kind: policy.kind, retainDays: keepForEver ? null : parsed })
        }
      >
        Save
      </Button>
    </Row>
  )
}

/** The Retention section: one row per artifact class from
 * `/api/v1/settings/retention`, with a days field and a Save per row.
 * An empty field keeps the class for ever. */
export function Retention({ api }: { api: ApiClient }) {
  const policies = useRetentionPolicies(api)
  return (
    <section className="settings-retention">
      <div className="settings-retention-title">
        <h1>Retention</h1>
        <span>How long the daemon keeps what sprites produce. Empty means for ever.</span>
      </div>
      <Frame
        hint={
          <>
            Memory is not here. Subject Pages are the sprite&apos;s memory and only
            Forget removes them; see the account card under Connections. A window
            counts from the day the item was made, and the sweep runs once a day
            at 3:00 in the Workspace timezone.
          </>
        }
      >
        {(policies.data ?? []).map((policy) => (
          <PolicyRow key={policy.kind} api={api} policy={policy} />
        ))}
      </Frame>
    </section>
  )
}
