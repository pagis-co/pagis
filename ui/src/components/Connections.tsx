// The Connection list and the connect flow (ADR-0022).
//
// The flow is four steps — provider, your Google client, Google, done —
// and the third step says plainly where the browser went and that the
// daemon waits on 127.0.0.1 for the answer. A cancelled or refused
// exchange returns to the second step with the entered values kept.
//
// Nothing typed here is stored: the client id and secret reach the
// daemon once, which hands them to the provider and keeps neither.
//
// The carrier account and the mail domain are Installation
// Connections: an administrator sets them up in the Administration
// Interface, and this page shows them without a way to change them.
// The carrier's row lists the numbers on it, which the person manages,
// and the mail domain's row says what the host cannot do and which
// Agents hold a mailbox on it.
//
// The picker draws the Provider Catalog the daemon serves
// (ADR-0012). This file holds no provider list: an entry says whether
// the setup is an OAuth exchange or a field list, what a Connection of
// it gives an Agent, and how many the Workspace may hold. The card a
// Connection gets follows its capabilities, not its provider name.

import { useEffect, useState } from 'react'
import { Plug } from 'lucide-react'
import { ConnectionRow } from './connection/ConnectionRow'
import { PageState } from './PageState'

import type { ApiClient, ConnectionDto, ProviderEntryDto } from '../api/client'
import { Button, Frame, Input } from '../primitives'
import {
  errorMessage,
  useAgentNames,
  useAuthorizeConnection,
  useConnectionProviders,
  useConnections,
  useCreateConnection,
  useDeleteConnection,
  useMailboxes,
  usePhoneNumbers,
  useReleasePhoneNumber,
} from '../queries'
import {
  AdoptNumber,
  type PhoneNumber,
  ReleaseConfirmation,
  formatE164,
} from './AgentPhoneNumber'
import {
  CapabilityPicker,
  READ_ONLY_CONNECTION_CAPABILITIES,
} from './ConnectionCapabilities'
import { AdministratorSetsUp } from './settings/AdministratorSetsUp'

import './Connections.css'
import './settings.css'

/** The capabilities a catalog entry declares. The card and the
 *  picker group follow these. */
const TELEPHONY = 'telephony'
const MAILBOXES = 'mailboxes'
const TEXTING = 'texting'

/** What the card says about a capability the provider does not have
 *  (ADR-0005, ADR-0020). The user reads it before a number is bought. */
const ABSENT_ENTRY_CAPABILITY: Record<string, string> = {
  [TEXTING]:
    'This carrier does not carry texts. Sprites on its numbers call only.',
}

function gives(connection: ConnectionDto, capability: string): boolean {
  return connection.capabilities.includes(capability)
}

/** What a Connection is, for the row that carries no account: the
 *  capability it gives, then the catalogue's label for its provider.
 *  The picker's catalogue holds no Installation Connection, so the
 *  carrier and the mail domain name themselves. */
function kindOf(
  connection: ConnectionDto,
  entry: ProviderEntryDto | undefined,
): string | undefined {
  if (gives(connection, TELEPHONY)) {
    return `Telephony carrier · ${entry?.label ?? connection.display_name}`
  }
  if (gives(connection, MAILBOXES)) {
    const host = entry?.label ?? connection.display_name
    const domain = connection.mail?.domain
    return domain == null
      ? `Mailbox provider · ${host}`
      : `Mailbox provider · ${host} · ${domain}`
  }
  return entry?.label
}

/** The picker group of one entry, by the capability it gives. A
 *  person picks only what a person connects, so every group is an
 *  account today. */
function groupOf(entry: ProviderEntryDto): string {
  if (entry.capabilities.includes('mail')) return 'Accounts'
  return 'Services'
}

/** The numbers the carrier carries: each live number with the
 *  Agent that holds it, so a spare number has a home besides every
 *  Agent's page. A spare number is released here; a held one is
 *  released from its Agent's page, which names the Agent that loses
 *  the line. A number the account already holds is added here too. */
function CarrierNumbers({ api }: { api: ApiClient }) {
  const page = usePhoneNumbers(api)
  const agentNames = useAgentNames(api)
  const release = useReleasePhoneNumber(api)
  const [adopting, setAdopting] = useState(false)
  const [releasing, setReleasing] = useState<PhoneNumber | null>(null)
  const numbers: PhoneNumber[] = (page.data?.items ?? []).filter(
    (number) => number.status !== 'released',
  )

  return (
    <div className="connection-mailboxes" data-testid="carrier-numbers">
      <span>Numbers</span>
      {numbers.length === 0 && (
        <span className="settings-hint">No numbers yet.</span>
      )}
      {numbers.map((number) => (
        <span className="connection-mailbox" key={number.id}>
          <strong>{formatE164(number.e164)}</strong>
          <span className="settings-hint">
            {number.agent_id != null
              ? `Held by ${agentNames[number.agent_id] ?? number.agent_id}`
              : 'Unassigned'}
          </span>
          {number.agent_id == null && (
            <Button
              variant="danger"
              size="sm"
              aria-label={`Release ${number.e164}`}
              onClick={() => setReleasing(number)}
            >
              Release
            </Button>
          )}
        </span>
      ))}
      {release.isError && (
        <p className="settings-error" role="alert">
          {errorMessage(release.error, 'That number could not be released.')}
        </p>
      )}
      {releasing !== null && (
        <ReleaseConfirmation
          number={releasing}
          holder={null}
          pending={release.isPending}
          onCancel={() => setReleasing(null)}
          onRelease={() =>
            release.mutate(releasing.id, { onSuccess: () => setReleasing(null) })
          }
        />
      )}
      {adopting ? (
        <AdoptNumber api={api} agent={null} onDone={() => setAdopting(false)} />
      ) : (
        <Button onClick={() => setAdopting(true)}>Add a number you own</Button>
      )}
    </div>
  )
}

/** The carrier card. The carrier account is the installation's, so
 *  the card changes nothing of it: it states the capabilities the
 *  carrier does not have (ADR-0020) and lists the numbers on it, which
 *  the person manages. */
function CarrierDetail({
  api,
  connection,
}: {
  api: ApiClient
  connection: ConnectionDto
}) {
  return (
    <div className="connection-row-detail">
      <AdministratorSetsUp api={api}>
        Numbers come from this installation's carrier account.
      </AdministratorSetsUp>
      {connection.absent_capabilities.length > 0 && (
        <ul className="settings-hint" data-testid="carrier-absent-capabilities">
          {connection.absent_capabilities.map((capability) => (
            <li key={capability}>
              {ABSENT_ENTRY_CAPABILITY[capability] ?? capability}
            </li>
          ))}
        </ul>
      )}
      <CarrierNumbers api={api} />
    </div>
  )
}

/** The connect flow of a `fields` entry: the name, the short
 *  name and the fields the entry declares, drawn from the catalog. A secret
 *  is masked and reaches the daemon once; the daemon proves what it
 *  can against the provider before it keeps anything. There is no
 *  browser step. */
export function ConnectFields({
  api,
  entry,
  onDone,
}: {
  api: ApiClient
  entry: ProviderEntryDto
  onDone: () => void
}) {
  const create = useCreateConnection(api)
  const [displayName, setDisplayName] = useState(entry.default_display_name)
  const [alias, setAlias] = useState(entry.default_alias)
  const [values, setValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(
      entry.fields.map((field) => [field.key, field.default ?? '']),
    ),
  )

  const ready =
    displayName.trim() !== '' &&
    alias.trim() !== '' &&
    entry.fields.every((field) => (values[field.key] ?? '').trim() !== '')

  return (
    <div className="connect-flow" aria-label={`Connect ${entry.label}`}>
      <section>
        <h4>Connect {entry.label}</h4>
        <p className="settings-hint">{entry.blurb}</p>
        <Input
          aria-label="Connection name"
          placeholder={`Connection name, e.g. ${entry.default_display_name}`}
          value={displayName}
          onChange={(event) => setDisplayName(event.target.value)}
        />
        <Input
          aria-label="Short name"
          placeholder={
            entry.default_alias === ''
              ? 'Short name sprites use'
              : `Short name, e.g. ${entry.default_alias}`
          }
          value={alias}
          onChange={(event) => setAlias(event.target.value)}
        />
        {entry.fields.map((field) => (
          <Input
            key={field.key}
            aria-label={field.label}
            placeholder={field.hint}
            type={
              field.secret
                ? 'password'
                : field.kind === 'number'
                  ? 'number'
                  : 'text'
            }
            value={values[field.key] ?? ''}
            onChange={(event) =>
              setValues((current) => ({
                ...current,
                [field.key]: event.target.value,
              }))
            }
          />
        ))}
        {create.isError && (
          <p className="settings-error" role="alert">
            {errorMessage(create.error, `${entry.label} could not be connected.`)}
          </p>
        )}
        <div className="settings-row-actions">
          <Button
            variant="primary"
            disabled={create.isPending || !ready}
            onClick={() =>
              create.mutate(
                {
                  provider: entry.id,
                  alias: alias.trim(),
                  display_name: displayName.trim(),
                  fields: Object.fromEntries(
                    entry.fields.map((field) => [
                      field.key,
                      (values[field.key] ?? '').trim(),
                    ]),
                  ),
                },
                { onSuccess: onDone },
              )
            }
          >
            Connect {entry.label}
          </Button>
          <Button onClick={onDone}>Cancel</Button>
        </div>
      </section>
    </div>
  )
}

/** What a mail host cannot do (ADR-0019). Each absent capability is
 *  shown with the work it leaves to the user. */
type MailCapability = 'idle' | 'outgoing_cap' | 'delete_mailbox' | 'reset_password'

const ABSENT_CAPABILITY: { key: MailCapability; absent: string }[] = [
  {
    key: 'idle',
    absent: 'This host does not wait for new mail. Pagis polls it instead.',
  },
  {
    key: 'outgoing_cap',
    absent: 'This host does not cap the sends. Pagis holds the Outgoing Cap.',
  },
  {
    key: 'delete_mailbox',
    absent: 'This host does not delete a mailbox. Delete it at the host.',
  },
  {
    key: 'reset_password',
    absent:
      'This host does not mint a password. Change it at the host and paste it.',
  },
]

/** The Mailbox Provider row (ADR-0019): the domain, the name,
 *  the status, the capabilities the host does not have, and the
 *  mailboxes on it with the Agent that holds each. A delete is refused
 *  inline while a mailbox points at the Connection. */
function MailboxProviderDetail({
  api,
  connection,
}: {
  api: ApiClient
  connection: ConnectionDto
}) {
  const mail = connection.mail
  const mailboxes = useMailboxes(api)
  const agentNames = useAgentNames(api)
  if (mail == null) return null

  const held = (mailboxes.data ?? []).filter(
    (mailbox) => mailbox.connection_id === connection.id,
  )
  const absent = ABSENT_CAPABILITY.filter((item) => !mail[item.key])

  return (
    <div className="connection-row-detail">
      <span className="connection-auth-mode">
        IMAP {mail.imap_host}:{mail.imap_port} · SMTP {mail.smtp_host}:
        {mail.smtp_port}
      </span>
      <AdministratorSetsUp api={api}>
        {mail.domain} is this installation's mail domain.
      </AdministratorSetsUp>

      {absent.length > 0 && (
        <ul className="settings-hint" data-testid="mailbox-absent-capabilities">
          {absent.map((item) => (
            <li key={item.key}>{item.absent}</li>
          ))}
        </ul>
      )}

      <div className="connection-mailboxes">
        <span>Mailboxes</span>
        {held.length === 0 && (
          <span className="settings-hint">None on this host yet.</span>
        )}
        {held.map((mailbox) => (
          <span className="connection-mailbox" key={mailbox.id}>
            {mailbox.address} · {agentNames[mailbox.agent_id] ?? mailbox.agent_id}
          </span>
        ))}
      </div>
    </div>
  )
}

/** What an account Connection adds under its row: the reconnect, and
 *  the capability widening that reconnect asks for. The Grants are not
 *  here: the connection page owns them, so each control exists once. */
function AccountDetail({
  api,
  connection,
  browserSignIn,
}: {
  api: ApiClient
  connection: ConnectionDto
  /** Whether the tab that goes to Google can ask for a sign-in to
   *  Pagis first: on a Server and in Remote Access, whose start
   *  route requires a Session of the Person. The catalog entry of the
   *  provider says so. */
  browserSignIn: boolean
}) {
  const authorize = useAuthorizeConnection(api)
  const currentCapabilities =
    connection.authorized_capabilities.length > 0
      ? connection.authorized_capabilities
      : READ_ONLY_CONNECTION_CAPABILITIES
  const [capabilities, setCapabilities] = useState<string[]>(currentCapabilities)
  const [widening, setWidening] = useState(false)
  const needsReconnect =
    connection.status === 'disconnected' ||
    connection.status === 'reauth_required'
  const brokered = connection.auth_mode === 'brokered'
  // A brokered connection sends the person to Google in their own
  // browser and the card waits for the redirect to land.
  const atGoogle = brokered && connection.status === 'connecting'
  const start = (capabilities: string[]) =>
    authorize.mutate(
      { connectionId: connection.id, capabilities },
      {
        onSuccess: (answer) => {
          setWidening(false)
          openAuthorization(answer.authorization_url)
        },
      },
    )

  return (
    <div className="connection-row-detail">
      <span className="connection-auth-mode">
        A sprite you grant this account can read and send from this inbox.
        Sprites know it as <code>{connection.alias}</code>.
      </span>
      <div className="settings-row-actions">
        <Button
          size="sm"
          aria-label={`${needsReconnect ? 'Reconnect' : 'Change access for'} ${connection.display_name}`}
          disabled={authorize.isPending || (!brokered && connection.status === 'connecting')}
          onClick={() => {
            if (needsReconnect) {
              start(currentCapabilities)
              return
            }
            setWidening((open) => !open)
          }}
        >
          {authorize.isPending
            ? 'Opening Google…'
            : needsReconnect
              ? 'Reconnect'
              : 'Change access'}
        </Button>
      </div>
      {atGoogle && (
        <p className="settings-hint" aria-live="polite">
          Finish signing in at Google in the tab that opened. This card
          turns connected when Google sends you back.
        </p>
      )}
      {widening && (
        <div className="connection-reauthorize">
          <CapabilityPicker selected={capabilities} onChange={setCapabilities} />
          <p className="settings-hint">
            {!brokered
              ? 'Pagis opens Google in your browser and waits on 127.0.0.1 for the answer.'
              : browserSignIn
                ? 'Pagis opens Google in a new tab. If that tab asks you to sign in to Pagis, sign in as yourself. Then grant the access at Google and come back to this page.'
                : 'Pagis opens Google in a new tab. Grant the access there and come back to this page.'}
          </p>
          <Button
            variant="primary"
            aria-label={`Authorize ${connection.display_name}`}
            disabled={authorize.isPending || capabilities.length === 0}
            onClick={() => start(capabilities)}
          >
            {authorize.isPending ? 'Opening Google…' : 'Continue at Google'}
          </Button>
        </div>
      )}
      {authorize.isError && (
        <p className="settings-error" role="alert">
          {errorMessage(authorize.error, 'Google did not complete this.')}
        </p>
      )}
    </div>
  )
}

type Step = 'client' | 'google' | 'done'

/** Open the start route that the authorize request answered. A brokered
 *  connection is finished in the person's own browser, on whatever
 *  machine they are on, so the client opens a tab and the record catches
 *  up. The start route sends a browser of this Person on to Google, and
 *  the Client App opens it in the system browser. */
function openAuthorization(url: string | null | undefined) {
  if (url != null && url !== '') window.open(url, '_blank', 'noopener')
}

/** The three-step connect flow. `onDone` closes it; the caller decides
 *  whether that returns to the list or finishes an onboarding step.
 *  The provider is picked before this flow starts.
 *
 *  The entry says which shape the form takes: a brokered
 *  installation declares the account field alone, because the Google
 *  client is the installation's and the person supplies nothing. */
export function ConnectGoogle({
  api,
  entry,
  onDone,
}: {
  api: ApiClient
  entry: ProviderEntryDto
  onDone: () => void
}) {
  const create = useCreateConnection(api)
  const authorize = useAuthorizeConnection(api)
  const remove = useDeleteConnection(api)
  const connections = useConnections(api)
  const refetchConnections = connections.refetch
  const [step, setStep] = useState<Step>('client')
  const [displayName, setDisplayName] = useState(entry.default_display_name)
  const [alias, setAlias] = useState('')
  const [account, setAccount] = useState('')
  const [clientId, setClientId] = useState('')
  const [clientSecret, setClientSecret] = useState('')
  const [connectionId, setConnectionId] = useState<string | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  const brokered = !entry.fields.some((field) => field.key === 'client_id')

  const ready =
    alias.trim() !== '' &&
    displayName.trim() !== '' &&
    account.trim() !== '' &&
    (brokered || (clientId.trim() !== '' && clientSecret !== ''))

  // The connection reaches `connected` when Google redirects the person
  // back to this installation, which is another request entirely. The
  // list is the one record of it, so the flow watches the list.
  const connected =
    connectionId !== null &&
    connections.data?.some(
      (connection) =>
        connection.id === connectionId && connection.status === 'connected',
    ) === true
  useEffect(() => {
    if (step === 'google' && connected) setStep('done')
  }, [step, connected])
  useEffect(() => {
    if (step !== 'google' || !brokered) return
    const poll = window.setInterval(() => {
      void refetchConnections()
    }, 1000)
    return () => window.clearInterval(poll)
  }, [step, brokered, refetchConnections])

  const connect = async () => {
    setFailure(null)
    let created
    try {
      created = await create.mutateAsync({
        provider: 'google',
        alias: alias.trim(),
        display_name: displayName.trim(),
        fields: brokered
          ? { account: account.trim() }
          : {
              account: account.trim(),
              client_id: clientId.trim(),
              client_secret: clientSecret,
            },
      })
    } catch (error) {
      setFailure(errorMessage(error, 'That account could not be recorded.'))
      return
    }
    setConnectionId(created.id)
    setStep('google')
    try {
      const answer = await authorize.mutateAsync({
        connectionId: created.id,
        capabilities: READ_ONLY_CONNECTION_CAPABILITIES,
      })
      openAuthorization(answer.authorization_url)
      // A local connection is finished when the request returns; a
      // brokered one waits for the redirect, which the list reports.
      if (answer.authorization_url == null) setStep('done')
    } catch (error) {
      // The record cannot be authorized and holds its alias, so the
      // second step starts over from the values the user still sees.
      await remove.mutateAsync(created.id).catch(() => undefined)
      setConnectionId(null)
      setFailure(errorMessage(error, 'Google did not complete the connection.'))
      setStep('client')
    }
  }

  return (
    <div className="connect-flow" aria-label="Connect Google">
      {step === 'client' && (
        <section>
          <h4>{brokered ? 'The account to connect' : 'Your Google client'}</h4>
          <p className="settings-hint">{entry.blurb}</p>
          <Input
            aria-label="Connection name"
            placeholder="Connection name, e.g. Work Google"
            value={displayName}
            onChange={(event) => setDisplayName(event.target.value)}
          />
          <Input
            aria-label="Short name"
            placeholder="Short name sprites use, e.g. work"
            value={alias}
            onChange={(event) => setAlias(event.target.value)}
          />
          <Input
            aria-label="Google account"
            placeholder="Google account, e.g. alice@example.com"
            value={account}
            onChange={(event) => setAccount(event.target.value)}
          />
          {!brokered && (
            <>
              <Input
                aria-label="Client ID"
                placeholder="Client ID"
                value={clientId}
                onChange={(event) => setClientId(event.target.value)}
              />
              <Input
                aria-label="Client secret"
                type="password"
                placeholder="Client secret"
                value={clientSecret}
                onChange={(event) => setClientSecret(event.target.value)}
              />
            </>
          )}
          {failure !== null && (
            <p className="settings-error" role="alert">
              {failure}
            </p>
          )}
          <div className="settings-row-actions">
            <Button
              variant="primary"
              disabled={!ready || create.isPending}
              onClick={() => void connect()}
            >
              Continue at Google
            </Button>
            <Button onClick={onDone}>Cancel</Button>
          </div>
        </section>
      )}

      {step === 'google' && (
        <section aria-live="polite">
          <h4>Finish at Google</h4>
          <p className="settings-hint">
            {brokered
              ? entry.browser_sign_in
                ? 'Google opened in a new tab. If that tab asks you to sign in to Pagis, sign in as yourself. Then sign in at Google and grant the read-only access Pagis asks for. This page turns over by itself when Google sends you back.'
                : 'Google opened in a new tab. Sign in there and grant the read-only access Pagis asks for. This page turns over by itself when Google sends you back.'
              : "Your browser has left for Google. Sign in there and grant the read-only access Pagis asks for. This page waits: Pagis is listening on 127.0.0.1 for Google's answer, and nothing leaves your machine except the sign-in itself."}
          </p>
        </section>
      )}

      {step === 'done' && (
        <section>
          <h4>{displayName.trim()} is connected</h4>
          <p className="settings-hint">
            Sprites know this account as <code>{alias.trim()}</code>, and
            reach it only with a grant. Give a sprite access on its page,
            or leave it for later. Read-only is what this account starts
            at — widen it from the connection card whenever you want.
          </p>
          <Button variant="primary" onClick={onDone}>Done</Button>
        </section>
      )}
    </div>
  )
}

/** What the page is doing: nothing, the picker, or one entry's flow. */
type Adding = { step: 'pick' } | { step: 'connect'; entry: ProviderEntryDto }

/** The picker: every catalog entry, grouped by what it gives an
 *  Agent. An entry the Workspace holds as many of as it allows is
 *  disabled with the reason. */
function ProviderPicker({
  entries,
  connections,
  onPick,
  onCancel,
}: {
  entries: ProviderEntryDto[]
  connections: ConnectionDto[]
  onPick: (entry: ProviderEntryDto) => void
  onCancel: () => void
}) {
  const groups = new Map<string, ProviderEntryDto[]>()
  for (const entry of entries) {
    const group = groupOf(entry)
    groups.set(group, [...(groups.get(group) ?? []), entry])
  }
  const held = (entry: ProviderEntryDto) =>
    connections.filter((connection) => connection.provider === entry.id).length
  const full = (entry: ProviderEntryDto) =>
    entry.max_instances != null && held(entry) >= entry.max_instances

  return (
    <div className="connect-flow" aria-label="Add a connection">
      <section>
        <h4>Add a connection</h4>
        {[...groups.entries()].map(([group, members]) => (
          <div className="connection-provider-group" key={group}>
            <span className="settings-hint">{group}</span>
            <div className="settings-row-actions">
              {members.map((entry) => (
                <Button
                  key={entry.id}
                  disabled={full(entry)}
                  onClick={() => onPick(entry)}
                >
                  {entry.label}
                </Button>
              ))}
            </div>
            {members.filter(full).map((entry) => (
              <span className="settings-hint" key={entry.id}>
                This workspace already has one {entry.label} connection, which
                is as many as it holds.
              </span>
            ))}
          </div>
        ))}
        <div className="settings-row-actions">
          <Button onClick={onCancel}>Cancel</Button>
        </div>
      </section>
    </div>
  )
}

export function Connections({
  api,
  onOpen,
}: {
  api: ApiClient
  /** The connection page one card opens. */
  onOpen: (connectionId: string) => void
}) {
  const connections = useConnections(api)
  const providers = useConnectionProviders(api)
  const remove = useDeleteConnection(api)
  const [adding, setAdding] = useState<Adding | null>(null)
  const entries = providers.data ?? []
  const entryOf = (connection: ConnectionDto) =>
    entries.find((entry) => entry.id === connection.provider)
  const close = () => setAdding(null)

  return (
    <div className="connections">
      <p className="settings-hint">Connect the accounts and services your sprites use. You choose which sprites have access.</p>
      {connections.isPending && <PageState icon={Plug} title="Loading connections…" />}
      {connections.isError && (
        <PageState icon={Plug} title="Could not load connections" onRetry={() => { void connections.refetch() }}>
          Check the connection to Pagis, then try again.
        </PageState>
      )}
      {(connections.data ?? []).length > 0 && (
        <Frame
          className="connection-list"
          hint="A sprite that needs a connection it does not have asks in its thread. You grant it from the connection's page, never from the sprite."
        >
          {(connections.data ?? []).map((connection) => (
            <ConnectionRow
              key={connection.id}
              api={api}
              connection={connection}
              kind={kindOf(connection, entryOf(connection))}
              testId={
                gives(connection, MAILBOXES) ? 'mailbox-provider-card' : undefined
              }
              onOpen={() => onOpen(connection.id)}
              onDelete={
                connection.installation
                  ? undefined
                  : () => remove.mutate(connection.id)
              }
              deleting={remove.isPending}
            >
              {gives(connection, TELEPHONY) ? (
                <CarrierDetail api={api} connection={connection} />
              ) : gives(connection, MAILBOXES) ? (
                <MailboxProviderDetail api={api} connection={connection} />
              ) : (
                <AccountDetail
                  api={api}
                  connection={connection}
                  browserSignIn={
                    entries.find((entry) => entry.id === connection.provider)
                      ?.browser_sign_in ?? false
                  }
                />
              )}
            </ConnectionRow>
          ))}
        </Frame>
      )}
      {remove.isError && (
        <p className="settings-error" role="alert">
          {errorMessage(remove.error, 'That connection could not be deleted.')}
        </p>
      )}
      {connections.isSuccess && connections.data.length === 0 && adding === null && (
        <PageState icon={Plug} title="Bring your tools together">
          No connections.
        </PageState>
      )}
      {adding?.step === 'pick' && (
        providers.isPending ? <PageState icon={Plug} title="Loading services…" /> :
        providers.isError ? <PageState icon={Plug} title="Could not load services" onRetry={() => { void providers.refetch() }} /> : <ProviderPicker
          entries={entries}
          connections={connections.data ?? []}
          onPick={(entry) => setAdding({ step: 'connect', entry })}
          onCancel={close}
        />
      )}
      {adding?.step === 'connect' &&
        (adding.entry.kind === 'oauth' ? (
          <ConnectGoogle api={api} entry={adding.entry} onDone={close} />
        ) : (
          <ConnectFields api={api} entry={adding.entry} onDone={close} />
        ))}
      {adding === null && (
        <div className="settings-row-actions">
          <Button variant="primary" onClick={() => setAdding({ step: 'pick' })}>
            Add a connection
          </Button>
        </div>
      )}
    </div>
  )
}
