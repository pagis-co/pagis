// The Agent Mailbox card and the mailbox fields both forms share
// (ADR-0019).
//
// The mailbox surfaces follow the phone number's: the card sits beside
// the number card on the Agent's access panel, and the fields appear in
// the Agent creation form, because the host makes the mailbox before
// the Agent exists and a host refusal belongs in that form.
//
// The card never shows the mailbox password and never lists mail: the
// desk is not a mail client. A delete takes the host's mail with it and
// Pagis keeps no copy, so the address is typed back first.
//
// The card also carries the Standing Mail Rule (ADR-0019): the
// one Event Subscription that wakes the Agent on its own mail. The
// Agent may narrow that rule with its own tools, and only the user
// pauses it. So the pause, the resume and the edit live here, and they
// go through the Event Subscription routes the Automations page uses.

import { useState } from 'react'

import type {
  AgentDto,
  ApiClient,
  ConnectionDto,
  SubscriptionDto,
} from '../api/client'
import { Button, Input, Select, Textarea } from '../primitives'
import {
  errorMessage,
  useAgentMailbox,
  useConnections,
  useDeleteMailbox,
  useMailboxName,
  useProvisionMailbox,
  useResetMailboxPassword,
  useSubscriptions,
  useUpdateSubscription,
  type NewMailboxBody,
} from '../queries'

import { AdministratorSetsUp } from './settings/AdministratorSetsUp'
import './AgentMailbox.css'
import './agent.css'
import './settings.css'

/** One Mailbox Provider the form can offer, and the free name this
 *  Agent would take on it. */
export interface MailboxOffer {
  connection_id: string
  display_name: string
  domain: string
  suggested_local_part: string
  default_outgoing_cap: number
  /** The host mints the mailbox password. Where it does not, the user
   *  makes the inbox at the host and pastes its password. */
  mints_password: boolean
  deletes_mailbox: boolean
}

/** What the two forms hold while the user types. The cap and the
 *  password are strings, because an empty field is not a number. */
export interface MailboxDraft {
  connectionId: string
  localPart: string
  outgoingCap: string
  password: string
}

/** The five states of the record (ADR-0019), in the words the user
 *  reads. */
const STATE_LABEL: Record<string, string> = {
  provisioning: 'Being made at the host',
  active: 'Active',
  unavailable: 'Unavailable',
  dormant: 'Sprite archived',
  deleted: 'Deleted',
}

/** The kind and the filter that make an Event Subscription the
 *  Standing Mail Rule of an Agent's own mailbox (ADR-0019). */
const MAIL_MESSAGE_RECEIVED = 'mail.message_received'
const OWN_MAILBOX = 'own'

/** The rule states the card shows. An archived rule is not one of
 *  them: the card holds the rule of a mailbox that stands. */
const RULE_STATE_LABEL: Record<string, string> = {
  active: 'Active',
  paused: 'Paused',
  blocked: 'Blocked',
}

/** The Agent's Standing Mail Rule among every Event Subscription. One
 *  mailbox has one such rule; an archived rule belongs to a mailbox
 *  that is gone. */
export function standingMailRuleOf(
  subscriptions: SubscriptionDto[],
  agentId: string,
): SubscriptionDto | undefined {
  return subscriptions.find(
    (subscription) =>
      subscription.agent_id === agentId &&
      subscription.event_kind === MAIL_MESSAGE_RECEIVED &&
      subscription.state !== 'archived' &&
      (subscription.filter as { mailbox?: string } | null)?.mailbox ===
        OWN_MAILBOX,
  )
}

/** Edit the instruction the rule wakes the Agent with. The daemon
 *  writes a new revision, as it does for a rule the Automations page
 *  edits. */
function EditStandingRule({
  api,
  rule,
  onDone,
}: {
  api: ApiClient
  rule: SubscriptionDto
  onDone: () => void
}) {
  const update = useUpdateSubscription(api)
  const [instruction, setInstruction] = useState(rule.instruction)

  return (
    <form
      className="mailbox-rule-edit"
      onSubmit={(event) => {
        event.preventDefault()
        update.mutate(
          {
            subscriptionId: rule.id,
            action: 'edit',
            instruction: instruction.trim(),
          },
          { onSuccess: onDone },
        )
      }}
    >
      <Textarea
        aria-label="Standing mail rule instruction"
        value={instruction}
        onChange={(event) => setInstruction(event.target.value)}
      />
      {update.isError && (
        <p className="settings-error" role="alert">
          {errorMessage(update.error, 'That rule could not be changed.')}
        </p>
      )}
      <div className="settings-row-actions">
        <Button onClick={onDone}>Cancel</Button>
        <Button
          type="submit"
          variant="primary"
          disabled={update.isPending || instruction.trim() === ''}
        >
          Save
        </Button>
      </div>
    </form>
  )
}

/** The Standing Mail Rule on the card (ADR-0019): what the Agent
 *  does with inbound mail, whether the rule stands, and the pause, the
 *  resume and the edit that only the user has. */
function StandingMailRule({
  api,
  agent,
}: {
  api: ApiClient
  agent: AgentDto
}) {
  const subscriptions = useSubscriptions(api)
  const update = useUpdateSubscription(api)
  const [editing, setEditing] = useState(false)

  if (subscriptions.data === undefined) return null
  const rule = standingMailRuleOf(subscriptions.data, agent.id)

  if (rule === undefined) {
    return (
      <p className="settings-warning" data-testid="standing-rule-missing">
        This mailbox has no standing rule, so mail that arrives wakes
        nobody.
      </p>
    )
  }

  return (
    <div className="mailbox-rule" data-testid="standing-mail-rule">
      <div className="mailbox-card-head">
        <strong>Standing mail rule</strong>
        <span
          className={
            rule.state === 'active'
              ? 'mailbox-chip'
              : 'mailbox-chip mailbox-chip-warning'
          }
          data-testid="standing-rule-state"
        >
          {RULE_STATE_LABEL[rule.state] ?? rule.state}
        </span>
      </div>
      <p data-testid="standing-rule-instruction">{rule.instruction}</p>
      {rule.state === 'paused' && (
        <p className="settings-hint" data-testid="standing-rule-paused">
          {agent.name} still reads and sends mail. It does not wake on
          mail that arrives.
        </p>
      )}
      {rule.blocked_reason != null && (
        <p className="settings-warning" role="alert">
          {rule.blocked_reason}
        </p>
      )}
      {update.isError && (
        <p className="settings-error" role="alert">
          {errorMessage(update.error, 'That rule could not be changed.')}
        </p>
      )}
      {!editing && (
        <div className="settings-row-actions">
          {rule.state === 'active' ? (
            <Button
              aria-label="Pause the standing mail rule"
              disabled={update.isPending}
              onClick={() =>
                update.mutate({ subscriptionId: rule.id, action: 'pause' })
              }
            >
              Pause
            </Button>
          ) : (
            <Button
              aria-label="Resume the standing mail rule"
              disabled={update.isPending}
              onClick={() =>
                update.mutate({ subscriptionId: rule.id, action: 'resume' })
              }
            >
              Resume
            </Button>
          )}
          <Button
            aria-label="Edit the standing mail rule"
            onClick={() => setEditing(true)}
          >
            Edit
          </Button>
        </div>
      )}
      {editing && (
        <EditStandingRule
          api={api}
          rule={rule}
          onDone={() => setEditing(false)}
        />
      )}
    </div>
  )
}

/** The draft a fresh section starts at, on the first offer. */
export function emptyDraft(offers: MailboxOffer[]): MailboxDraft {
  const offer = offers[0]
  return {
    connectionId: offer?.connection_id ?? '',
    localPart: offer?.suggested_local_part ?? '',
    outgoingCap: '',
    password: '',
  }
}

export function offerOf(
  offers: MailboxOffer[],
  connectionId: string,
): MailboxOffer | undefined {
  return offers.find((offer) => offer.connection_id === connectionId)
}

/** The request body of a draft. An empty cap takes the host's default,
 *  and a password only travels where the host mints none. */
export function mailboxBody(
  draft: MailboxDraft,
  offer: MailboxOffer,
): NewMailboxBody {
  const cap = Number.parseInt(draft.outgoingCap, 10)
  return {
    connection_id: draft.connectionId,
    local_part: draft.localPart.trim(),
    outgoing_cap: Number.isNaN(cap) ? undefined : cap,
    password: offer.mints_password ? undefined : draft.password,
  }
}

/** Whether the draft is complete enough to send. The daemon is the
 *  authority on the name; this only stops an empty request. */
export function draftIsReady(
  draft: MailboxDraft,
  offer: MailboxOffer | undefined,
): boolean {
  if (offer === undefined || draft.localPart.trim() === '') return false
  return offer.mints_password || draft.password !== ''
}

/** The mailbox fields: the provider, the name beside the domain with
 *  its live Address Ledger check, the Outgoing Cap, and the password of
 *  an inbox made at a manual host (ADR-0019). */
export function MailboxFields({
  api,
  offers,
  draft,
  onChange,
}: {
  api: ApiClient
  offers: MailboxOffer[]
  draft: MailboxDraft
  onChange: (draft: MailboxDraft) => void
}) {
  const offer = offerOf(offers, draft.connectionId)
  const localPart = draft.localPart.trim()
  const check = useMailboxName(
    api,
    draft.connectionId === '' ? null : draft.connectionId,
    localPart,
  )

  return (
    <div className="mailbox-fields">
      {offers.length > 1 && (
        <Select
          label="Mailbox provider"
          value={draft.connectionId}
          onValueChange={(value) => onChange({ ...draft, connectionId: value })}
          items={offers.map((item) => ({
            value: item.connection_id,
            label: `${item.display_name} · ${item.domain}`,
          }))}
        />
      )}

      <div className="mailbox-address-field">
        <Input
          aria-label="Mailbox name"
          placeholder="Name before the @"
          value={draft.localPart}
          onChange={(event) =>
            onChange({ ...draft, localPart: event.target.value })
          }
        />
        <span className="mailbox-domain">@{offer?.domain ?? ''}</span>
      </div>

      {check.data !== undefined && localPart !== '' && (
        <p
          className={check.data.available ? 'settings-hint' : 'settings-warning'}
          data-testid="mailbox-name-check"
        >
          {check.data.available
            ? `${check.data.address} is free.`
            : (check.data.reason ?? `${check.data.address} cannot be used.`)}
        </p>
      )}

      <Input
        aria-label="Outgoing cap"
        type="number"
        min={1}
        placeholder={`Messages a day, e.g. ${offer?.default_outgoing_cap ?? 20}`}
        value={draft.outgoingCap}
        onChange={(event) =>
          onChange({ ...draft, outgoingCap: event.target.value })
        }
      />

      {offer !== undefined && !offer.mints_password && (
        <>
          <p className="settings-hint">
            This mail host has no API. Make the inbox at the host first,
            then paste its password here. The daemon stores the password
            encrypted and the desk never shows it again.
          </p>
          <Input
            aria-label="Mailbox password"
            type="password"
            placeholder="Mailbox password"
            value={draft.password}
            onChange={(event) =>
              onChange({ ...draft, password: event.target.value })
            }
          />
        </>
      )}
    </div>
  )
}

/** The line the creation form shows where the installation holds no
 *  mail domain. The domain is the installation's, so the line says who
 *  sets it up. */
export function NoMailboxProvider({ api }: { api: ApiClient }) {
  return (
    <AdministratorSetsUp api={api} testId="no-mailbox-provider">
      This installation has no mail domain yet, so no sprite can have an
      address of its own.
    </AdministratorSetsUp>
  )
}

/** Delete a mailbox with the address typed back (ADR-0019). */
function DeleteConfirmation({
  address,
  deletesAtHost,
  pending,
  onCancel,
  onDelete,
}: {
  address: string
  deletesAtHost: boolean
  pending: boolean
  onCancel: () => void
  onDelete: () => void
}) {
  const [typed, setTyped] = useState('')
  return (
    <div className="mailbox-delete" role="dialog" aria-label="Delete mailbox">
      <p>
        The mail in {address} goes with the mailbox. Pagis keeps no copy of
        a message, so nothing here comes back. Type the address to confirm.
      </p>
      {!deletesAtHost && (
        <p className="settings-warning">
          This mail host has no API. Pagis forgets the mailbox; delete the
          inbox at the host yourself.
        </p>
      )}
      <Input
        aria-label="Type the address to confirm"
        placeholder={address}
        value={typed}
        onChange={(event) => setTyped(event.target.value)}
      />
      <div className="settings-row-actions">
        <Button onClick={onCancel}>Keep it</Button>
        <Button
          variant="danger"
          aria-label={`Delete ${address}`}
          disabled={pending || typed.trim() !== address}
          onClick={onDelete}
        >
          Delete it
        </Button>
      </div>
    </div>
  )
}

/** Prove the login again, with a pasted password where the host mints
 *  none. The cursor does not move, so nothing that arrived while the
 *  mailbox was unavailable is lost. */
function ResetPassword({
  api,
  agent,
  mintsPassword,
  onDone,
}: {
  api: ApiClient
  agent: AgentDto
  mintsPassword: boolean
  onDone: () => void
}) {
  const reset = useResetMailboxPassword(api)
  const [password, setPassword] = useState('')

  return (
    <div className="mailbox-reset">
      {mintsPassword ? (
        <p className="settings-hint">
          Pagis asks the host for a new password and proves the login
          again. Mail already in the mailbox is untouched.
        </p>
      ) : (
        <>
          <p className="settings-hint">
            This mail host has no API. Change the password at the host,
            then paste the new one here.
          </p>
          <Input
            aria-label="New mailbox password"
            type="password"
            placeholder="New mailbox password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
          />
        </>
      )}
      {reset.isError && (
        <p className="settings-error" role="alert">
          {errorMessage(reset.error, 'The password could not be reset.')}
        </p>
      )}
      <div className="settings-row-actions">
        <Button onClick={onDone}>Cancel</Button>
        <Button
          variant="primary"
          aria-label="Reset the mailbox password"
          disabled={reset.isPending || (!mintsPassword && password === '')}
          onClick={() =>
            reset.mutate(
              {
                agentId: agent.id,
                password: mintsPassword ? undefined : password,
              },
              {
                onSuccess: () => {
                  setPassword('')
                  onDone()
                },
              },
            )
          }
        >
          Reset password
        </Button>
      </div>
    </div>
  )
}

/** Give an Agent that holds none a mailbox, through the same path the
 *  creation form uses. */
function ProvisionMailbox({
  api,
  agent,
  offers,
  onDone,
}: {
  api: ApiClient
  agent: AgentDto
  offers: MailboxOffer[]
  onDone: () => void
}) {
  const provision = useProvisionMailbox(api)
  const [draft, setDraft] = useState<MailboxDraft>(() => emptyDraft(offers))
  const offer = offerOf(offers, draft.connectionId)

  return (
    <div className="mailbox-provision" aria-label={`Provision a mailbox for ${agent.name}`}>
      <MailboxFields
        api={api}
        offers={offers}
        draft={draft}
        onChange={setDraft}
      />
      {provision.isError && (
        <p className="settings-error" role="alert">
          {errorMessage(provision.error, 'That mailbox could not be made.')}
        </p>
      )}
      <div className="settings-row-actions">
        <Button onClick={onDone}>Cancel</Button>
        <Button
          variant="primary"
          aria-label={`Make the mailbox for ${agent.name}`}
          disabled={provision.isPending || !draftIsReady(draft, offer)}
          onClick={() => {
            if (offer === undefined) return
            provision.mutate(
              { agentId: agent.id, ...mailboxBody(draft, offer) },
              { onSuccess: onDone },
            )
          }}
        >
          {provision.isPending ? 'Making it…' : 'Make the mailbox'}
        </Button>
      </div>
    </div>
  )
}

export function AgentMailbox({
  api,
  agent,
}: {
  api: ApiClient
  agent: AgentDto
}) {
  const page = useAgentMailbox(api, agent.id)
  const connections = useConnections(api)
  const remove = useDeleteMailbox(api)
  const [provisioning, setProvisioning] = useState(false)
  const [resetting, setResetting] = useState(false)
  const [deleting, setDeleting] = useState(false)

  const mailbox = page.data?.mailbox ?? null
  const offers = (page.data?.offers ?? []) as MailboxOffer[]
  const provider: ConnectionDto | undefined = (connections.data ?? []).find(
    (connection) => connection.id === mailbox?.connection_id,
  )
  // The host's own capabilities decide what the card offers: a host
  // that mints no password takes a pasted one, and a host that deletes
  // no mailbox leaves the inbox behind (ADR-0019).
  const mintsPassword = provider?.mail?.reset_password ?? true
  const deletesAtHost = provider?.mail?.delete_mailbox ?? true

  return (
    <section className="agent-mailbox" aria-label={`${agent.name} mailbox`}>
      <div className="agent-section-header">
        <h4>Mailbox</h4>
        {mailbox === null && !provisioning && offers.length > 0 && (
          <Button onClick={() => setProvisioning(true)}>Provision a mailbox</Button>
        )}
      </div>
      <p className="settings-hint">
        {agent.name} reads and sends mail from one address of its own. The
        desk never shows its password and never lists its mail.
      </p>

      {mailbox === null && offers.length === 0 && !provisioning && (
        <NoMailboxProvider api={api} />
      )}

      {mailbox === null && provisioning && (
        <ProvisionMailbox
          api={api}
          agent={agent}
          offers={offers}
          onDone={() => setProvisioning(false)}
        />
      )}

      {mailbox !== null && (
        <div className="mailbox-card" data-testid="agent-mailbox-card">
          <div className="mailbox-card-head">
            <strong>{mailbox.address}</strong>
            <span
              className={
                mailbox.state === 'active'
                  ? 'mailbox-chip'
                  : 'mailbox-chip mailbox-chip-warning'
              }
              data-testid="mailbox-state"
            >
              {STATE_LABEL[mailbox.state] ?? mailbox.state}
            </span>
          </div>
          {mailbox.reason != null && (
            <p className="settings-warning" role="alert">
              {mailbox.reason}
            </p>
          )}
          <p className="settings-hint" data-testid="mailbox-cap">
            Outgoing cap {mailbox.outgoing_cap} a day · {mailbox.sends_today}{' '}
            sent today.
          </p>
          <StandingMailRule api={api} agent={agent} />
          {remove.isError && (
            <p className="settings-error" role="alert">
              {errorMessage(remove.error, 'That mailbox could not be deleted.')}
            </p>
          )}
          <div className="settings-row-actions">
            {mailbox.state === 'unavailable' && !resetting && (
              <Button onClick={() => setResetting(true)}>Reset password</Button>
            )}
            {!deleting && (
              <Button
                variant="danger"
                aria-label={`Delete the mailbox of ${agent.name}`}
                onClick={() => setDeleting(true)}
              >
                Delete mailbox
              </Button>
            )}
          </div>
          {resetting && (
            <ResetPassword
              api={api}
              agent={agent}
              mintsPassword={mintsPassword}
              onDone={() => setResetting(false)}
            />
          )}
          {deleting && (
            <DeleteConfirmation
              address={mailbox.address}
              deletesAtHost={deletesAtHost}
              pending={remove.isPending}
              onCancel={() => setDeleting(false)}
              onDelete={() =>
                remove.mutate(
                  { agentId: agent.id, confirmAddress: mailbox.address },
                  { onSuccess: () => setDeleting(false) },
                )
              }
            />
          )}
        </div>
      )}
    </section>
  )
}
