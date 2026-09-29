// The installation's setup of every provider: the model keys, the
// Google OAuth client, the carrier account with its SIP sign-in, and
// the mail domain.
//
// The daemon declares each provider's installation parts and serves
// them with the fields each part asks for, so this page holds no
// provider list and no form of its own: a provider added to the
// catalog shows here with no change. What a person does with a
// provider (their own Google account, an Agent Phone Number, an Agent
// Mailbox) stays in the product.

import { useState } from 'react'

import type { ApiClient, ProviderSetupDto, SetupPartDto } from '../api/client'
import { Badge, Button, Frame, Input, Row, SectionLabel } from '../primitives'
import {
  errorMessage,
  useConfigureProviderPart,
  useProviderSetups,
  useRemoveProviderPart,
  useTestProviderPart,
} from '../queries'

/** The groups, in the order the page shows them. */
const GROUPS: { value: string; label: string }[] = [
  { value: 'models', label: 'Model providers' },
  { value: 'accounts', label: 'Accounts' },
  { value: 'telephony', label: 'Phone carriers' },
  { value: 'mailboxes', label: 'Mail domains' },
]

function badge(part: SetupPartDto) {
  if (!part.configured) return <Badge tone="neutral">Not set up</Badge>
  const status = part.status ?? null
  if (status === null || status === 'connected') return <Badge tone="working">Set up</Badge>
  return <Badge tone="failed">{status.replace('_', ' ')}</Badge>
}

/** The fields the form asks for. A Connection that exists takes a new
 *  secret and keeps the rest, so the form asks for the secret alone. */
function formFields(part: SetupPartDto) {
  if (part.kind === 'connection' && part.configured) {
    return part.fields.filter((field) => field.secret)
  }
  return part.fields
}

function PartForm({
  api,
  provider,
  part,
  onDone,
}: {
  api: ApiClient
  provider: ProviderSetupDto
  part: SetupPartDto
  onDone: () => void
}) {
  const configure = useConfigureProviderPart(api)
  const fields = formFields(part)
  const [values, setValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(fields.map((field) => [field.key, field.default ?? ''])),
  )
  const ready = fields.every((field) => (values[field.key] ?? '').trim() !== '')

  return (
    <div className="administration-form" aria-label={`Set up ${provider.label} ${part.label}`}>
      <p className="administration-note">{part.blurb}</p>
      {fields.map((field) => (
        <label key={field.key}>
          {field.label}
          <Input
            aria-label={`${provider.label} ${field.label}`}
            placeholder={field.hint}
            autoComplete="off"
            type={field.secret ? 'password' : field.kind === 'number' ? 'number' : 'text'}
            value={values[field.key] ?? ''}
            onChange={(event) =>
              setValues((current) => ({ ...current, [field.key]: event.target.value }))
            }
          />
        </label>
      ))}
      {configure.isError && (
        <p className="administration-error" role="alert">
          {errorMessage(configure.error, `${provider.label} did not accept that.`)}
        </p>
      )}
      <div className="administration-actions">
        <Button
          variant="primary"
          disabled={configure.isPending || !ready}
          onClick={() =>
            configure.mutate(
              {
                provider: provider.provider,
                part: part.id,
                fields: Object.fromEntries(
                  fields.map((field) => [
                    field.key,
                    field.secret ? (values[field.key] ?? '') : (values[field.key] ?? '').trim(),
                  ]),
                ),
              },
              { onSuccess: onDone },
            )
          }
        >
          Save
        </Button>
        <Button onClick={onDone}>Cancel</Button>
      </div>
    </div>
  )
}

function PartRow({
  api,
  provider,
  part,
  blockedBy,
}: {
  api: ApiClient
  provider: ProviderSetupDto
  part: SetupPartDto
  /** The part this one needs first, when that part is not set up. */
  blockedBy: SetupPartDto | null
}) {
  const [editing, setEditing] = useState(false)
  const test = useTestProviderPart(api)
  const remove = useRemoveProviderPart(api)
  const ref = { provider: provider.provider, part: part.id }
  const name = `${provider.label} ${part.label}`

  return (
    <Row className="administration-part">
      <div className="administration-part-line">
        <span className="administration-key">{part.label}</span>
        {badge(part)}
        {part.facts.map((fact) => (
          <span className="administration-note" key={fact.label}>
            {fact.label}: <span className="administration-mono">{fact.value}</span>
          </span>
        ))}
        <span className="administration-row-trailing administration-actions">
          {blockedBy !== null ? (
            <span className="administration-note">Set up the {blockedBy.label} first.</span>
          ) : (
            <Button
              aria-label={`${part.configured ? 'Replace' : 'Set up'} the ${name}`}
              onClick={() => setEditing((open) => !open)}
            >
              {part.configured ? 'Replace' : 'Set up'}
            </Button>
          )}
          {part.configured && part.testable && (
            <Button
              aria-label={`Test the ${name}`}
              disabled={test.isPending}
              onClick={() => test.mutate(ref)}
            >
              Test
            </Button>
          )}
          {part.configured && (
            <Button
              variant="danger-quiet"
              aria-label={`Remove the ${name}`}
              disabled={remove.isPending}
              onClick={() => remove.mutate(ref)}
            >
              Remove
            </Button>
          )}
        </span>
      </div>
      {test.isSuccess && !test.isPending && (
        <p className="administration-note" role="status">
          {provider.label} accepted the kept credential.
        </p>
      )}
      {test.isError && (
        <p className="administration-error" role="alert">
          {errorMessage(test.error, `${provider.label} did not accept the kept credential.`)}
        </p>
      )}
      {remove.isError && (
        <p className="administration-error" role="alert">
          {errorMessage(remove.error, `The ${name} could not be removed.`)}
        </p>
      )}
      {editing && (
        <PartForm api={api} provider={provider} part={part} onDone={() => setEditing(false)} />
      )}
    </Row>
  )
}

function ProviderFrame({ api, provider }: { api: ApiClient; provider: ProviderSetupDto }) {
  // A SIP sign-in signs in to a carrier account, so it waits for one.
  const connection = provider.parts.find((part) => part.kind === 'connection') ?? null
  return (
    <>
      <SectionLabel>{provider.label}</SectionLabel>
      <Frame>
        {provider.parts.map((part) => (
          <PartRow
            key={part.id}
            api={api}
            provider={provider}
            part={part}
            blockedBy={
              part.kind === 'sip_credential' && connection !== null && !connection.configured
                ? connection
                : null
            }
          />
        ))}
      </Frame>
    </>
  )
}

export function Providers({ api }: { api: ApiClient }) {
  const setups = useProviderSetups(api)

  return (
    <section className="administration-section">
      <div className="administration-title">
        <h2>Providers</h2>
        <span>
          What this installation sets up one time for everybody. Each person
          connects their own accounts, numbers and mailboxes in Pagis.
        </span>
      </div>
      {!setups.data ? (
        <p className="administration-note">
          {setups.isError ? 'The providers could not be read.' : 'Reading…'}
        </p>
      ) : (
        GROUPS.map((group) => {
          const members = (setups.data ?? []).filter((setup) => setup.group === group.value)
          if (members.length === 0) return null
          return (
            <div className="administration-group" key={group.value}>
              <h3>{group.label}</h3>
              {members.map((provider) => (
                <ProviderFrame key={provider.provider} api={api} provider={provider} />
              ))}
            </div>
          )
        })
      )}
    </section>
  )
}
