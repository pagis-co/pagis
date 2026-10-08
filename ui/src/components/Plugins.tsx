// The Plugins view of the Administration Interface (ADR-0017): the
// installed rows, each with what it ships and what it is bound to, and
// the Install card. A Plugin is the Org's, so installing, updating,
// binding and removing one answer on the administration port alone.
//
// A fetch is the install itself, because only the daemon can read the
// package. The card shows the answer as a summary, binds the account
// the package asks for, and asks the user to accept the tools that
// never ask. Cancel removes the fetched package again, so nothing
// stays behind a card the user did not keep.

import { useState } from 'react'

import type {
  ApiClient,
  BindingValueRequest,
  ConnectionDto,
  PluginDto,
  PluginFieldDto,
  PluginRowDto,
} from '../api/client'
import { Badge, Button, Frame, Input, Row, SectionLabel, Select } from '../primitives'
import {
  errorMessage,
  useBindPluginField,
  useConnections,
  useInstallPlugin,
  usePlugin,
  usePlugins,
  useUninstallPlugin,
  useUpdatePlugin,
} from '../queries'

import './Plugins.css'

/** A tool of this class asks nobody, so the user accepts it at
 *  install: a Plugin may raise its class alone, never lower it. */
const LOWERED_EFFECT = 'free'

function sentenceList(items: string[]): string {
  if (items.length <= 1) return items.join('')
  return `${items.slice(0, -1).join(', ')} and ${items[items.length - 1]}`
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? '' : 's'}`
}

function capitalize(word: string): string {
  return word.charAt(0).toUpperCase() + word.slice(1)
}

/** The tools a Plugin offers: the frozen manifest once its servers
 *  have been read, and what the package declares before that. */
function toolCount(plugin: PluginDto): number {
  return plugin.frozen_tools.length > 0 ? plugin.frozen_tools.length : plugin.tools.length
}

/** The required fields that hold no Binding. They are why a Plugin is
 *  disabled (ADR-0017). */
function missingFields(plugin: PluginDto): PluginFieldDto[] {
  return plugin.fields.filter(
    (field) =>
      field.required && !plugin.bindings.some((binding) => binding.field === field.name),
  )
}

/** What the row says about the accounts: the bound one, what still
 *  waits, or that the Plugin needs nothing. */
function bindingSentence(plugin: PluginDto, connections: ConnectionDto[]): string {
  const bound = plugin.bindings.find((binding) => binding.kind === 'connection')
  if (bound !== undefined) {
    const connection = connections.find((item) => item.id === bound.connection_id)
    const provider =
      connection === undefined ? 'an account' : capitalize(connection.provider)
    const account = connection?.account ?? connection?.display_name ?? bound.connection_id
    return `Bound to ${provider} · ${account}`
  }
  const missing = missingFields(plugin)
  if (missing.length > 0)
    return `Needs ${sentenceList(missing.map((field) => field.title))}`
  return 'Needs nothing from you'
}

export function stateBadge(plugin: PluginDto) {
  if (plugin.state === 'failed') return <Badge tone="failed">Failed</Badge>
  if (plugin.tools_changed) return <Badge tone="waiting">Update available</Badge>
  if (plugin.state === 'disabled') return <Badge>Disabled</Badge>
  return <Badge tone="working">Up to date</Badge>
}

/** One installed Plugin. The row reads the Plugin itself: the list
 *  carries the record alone, and the counts and the Bindings are the
 *  Plugin's. */
function PluginRow({ api, row }: { api: ApiClient; row: PluginRowDto }) {
  const held = usePlugin(api, row.id)
  const connections = useConnections(api)
  const update = useUpdatePlugin(api, row.id)
  const uninstall = useUninstallPlugin(api)
  const [confirming, setConfirming] = useState(false)
  const plugin = held.data

  return (
    <Row className="plugin-row">
      <span className="plugin-tile" aria-hidden="true">
        {row.name.charAt(0).toUpperCase()}
      </span>
      <span className="plugin-name">
        <strong>{row.name}</strong>
        {plugin !== undefined && (
          <span className="plugin-counts">
            v{plugin.manifest_version} ·{' '}
            {plugin.servers.length === 0
              ? 'no server'
              : `Servers ${plugin.servers.length}`}{' '}
            · Tools {toolCount(plugin)} · Skills {plugin.skills.length}
          </span>
        )}
      </span>
      {plugin !== undefined && stateBadge(plugin)}
      <span className="plugin-binding">
        {plugin !== undefined && bindingSentence(plugin, connections.data ?? [])}
      </span>
      {confirming ? (
        <>
          <Button
            variant="danger"
            size="sm"
            aria-label={`Uninstall ${row.name} for good`}
            disabled={uninstall.isPending}
            onClick={() => uninstall.mutate(row.id)}
          >
            Uninstall for good
          </Button>
          <Button variant="ghost" size="sm" onClick={() => setConfirming(false)}>
            Keep it
          </Button>
        </>
      ) : (
        <>
          <Button
            size="sm"
            aria-label={`Update ${row.name}`}
            disabled={update.isPending}
            onClick={() => update.mutate(undefined)}
          >
            Update
          </Button>
          <Button
            variant="ghost"
            size="sm"
            aria-label={`Uninstall ${row.name}`}
            onClick={() => setConfirming(true)}
          >
            Uninstall
          </Button>
        </>
      )}
      {(update.isError || uninstall.isError) && (
        <span className="settings-error plugin-row-error" role="alert">
          {update.isError
            ? errorMessage(update.error, 'That update was refused.')
            : errorMessage(uninstall.error, 'That plugin was not removed.')}
        </span>
      )}
    </Row>
  )
}

/** One field of the fetched package: the account for a connection
 *  field, and a value for every other kind. */
function FieldControl({
  field,
  draft,
  accounts,
  onChange,
}: {
  field: PluginFieldDto
  draft: string
  accounts: ConnectionDto[]
  onChange: (value: string) => void
}) {
  if (field.kind === 'connection') {
    return (
      <Select
        label={field.title}
        value={draft}
        onValueChange={onChange}
        placeholder="Choose an account"
        className="plugin-account"
        items={accounts.map((connection) => ({
          value: connection.id,
          label: connection.display_name,
        }))}
      />
    )
  }
  return (
    <Input
      aria-label={field.title}
      placeholder={field.title}
      type={field.kind === 'secret' ? 'password' : 'text'}
      value={draft}
      onChange={(event) => onChange(event.target.value)}
    />
  )
}

function bindingValue(field: PluginFieldDto, draft: string): BindingValueRequest {
  if (field.kind === 'connection') return { kind: 'connection', connection_id: draft }
  if (field.kind === 'secret') return { kind: 'secret', secret: draft }
  if (field.kind === 'number') return { kind: 'value', value: Number(draft) }
  if (field.kind === 'boolean') return { kind: 'value', value: draft === 'true' }
  return { kind: 'value', value: draft }
}

/** The fetched package: what it ships, what it binds, and the consent
 *  to the tools that never ask. Install writes the Bindings and keeps
 *  the Plugin. Cancel removes it. */
function PackageSummary({
  api,
  plugin,
  onDone,
}: {
  api: ApiClient
  plugin: PluginDto
  onDone: () => void
}) {
  const connections = useConnections(api)
  const bind = useBindPluginField(api, plugin.id)
  const uninstall = useUninstallPlugin(api)
  const [drafts, setDrafts] = useState<Record<string, string>>({})
  const [accepted, setAccepted] = useState(false)
  const [failure, setFailure] = useState<string | null>(null)

  const lowered = plugin.tools.filter((tool) => tool.effect === LOWERED_EFFECT)
  const required = plugin.fields.filter((field) => field.required)
  const account = plugin.fields.find((field) => field.kind === 'connection')
  const ready =
    required.every((field) => (drafts[field.name] ?? '').trim() !== '') &&
    (lowered.length === 0 || accepted)
  const busy = bind.isPending || uninstall.isPending

  const summary = [
    plural(plugin.servers.length, 'server'),
    plural(toolCount(plugin), 'tool'),
    plural(plugin.skills.length, 'skill'),
    account === undefined
      ? null
      : `binds a${account.provider == null ? 'n' : ` ${capitalize(account.provider)}`} account`,
  ]
    .filter((part) => part !== null)
    .join(' · ')

  const install = async () => {
    setFailure(null)
    try {
      for (const field of plugin.fields) {
        const draft = drafts[field.name] ?? ''
        if (draft.trim() === '') continue
        await bind.mutateAsync({
          field: field.name,
          value: bindingValue(field, draft),
        })
      }
    } catch (error) {
      setFailure(errorMessage(error, 'That value was not accepted.'))
      return
    }
    onDone()
  }

  return (
    <>
      <div className="plugin-summary">
        <strong>
          {plugin.name} {plugin.manifest_version}
        </strong>
        <span className="plugin-summary-counts">{summary}</span>
        {plugin.fields.map((field) => (
          <FieldControl
            key={field.name}
            field={field}
            draft={drafts[field.name] ?? ''}
            accounts={(connections.data ?? []).filter(
              (connection) =>
                field.provider == null || connection.provider === field.provider,
            )}
            onChange={(value) => setDrafts((held) => ({ ...held, [field.name]: value }))}
          />
        ))}
      </div>
      {lowered.length > 0 && (
        <label className="plugin-accept">
          <input
            type="checkbox"
            checked={accepted}
            onChange={(event) => setAccepted(event.target.checked)}
          />
          Accept the tools that never ask you
        </label>
      )}
      <div className="plugin-actions">
        <Button
          variant="primary"
          size="sm"
          disabled={busy || !ready}
          onClick={() => void install()}
        >
          Install
        </Button>
        <Button
          variant="ghost"
          size="sm"
          disabled={busy}
          onClick={() => uninstall.mutate(plugin.id, { onSuccess: onDone })}
        >
          Cancel
        </Button>
      </div>
      {(failure !== null || uninstall.isError) && (
        <p className="settings-error" role="alert">
          {failure ?? errorMessage(uninstall.error, 'That plugin was not removed.')}
        </p>
      )}
    </>
  )
}

/** The Install card: the fetch field, and the package once the
 *  daemon has read it. */
function InstallCard({ api, onDone }: { api: ApiClient; onDone: () => void }) {
  const install = useInstallPlugin(api)
  const [url, setUrl] = useState('')
  const [fetched, setFetched] = useState<PluginDto | null>(null)

  return (
    <Frame>
      <div className="plugin-install">
        <div className="plugin-fetch">
          <Input
            aria-label="Package name or git URL"
            placeholder="Package name or git URL"
            className="plugin-fetch-field"
            value={url}
            onChange={(event) => setUrl(event.target.value)}
            disabled={fetched !== null}
          />
          <Button
            variant="primary"
            size="sm"
            disabled={install.isPending || fetched !== null || url.trim() === ''}
            onClick={() =>
              install.mutate({ kind: 'git', url: url.trim() }, { onSuccess: setFetched })
            }
          >
            Fetch
          </Button>
        </div>
        {install.isError && (
          <p className="settings-error" role="alert">
            {errorMessage(install.error, 'That package was not read.')}
          </p>
        )}
        {fetched === null ? (
          <div className="plugin-actions">
            <Button variant="ghost" size="sm" onClick={onDone}>
              Cancel
            </Button>
          </div>
        ) : (
          <PackageSummary api={api} plugin={fetched} onDone={onDone} />
        )}
      </div>
    </Frame>
  )
}

export function Plugins({ api }: { api: ApiClient }) {
  const plugins = usePlugins(api)
  const [installing, setInstalling] = useState(false)

  return (
    <div className="plugins">
      <div className="plugin-title">
        <h3>Plugins</h3>
        <span className="plugin-lead">
          Servers, tools and skills every sprite of the installation can use. A plugin
          binds to your accounts, never the other way.
        </span>
        <Button
          variant="primary"
          size="sm"
          className="plugin-title-action"
          disabled={installing}
          onClick={() => setInstalling(true)}
        >
          Install a plugin
        </Button>
      </div>
      {plugins.data?.length === 0 && (
        <p className="plugin-empty">No plugin is installed.</p>
      )}
      {plugins.data !== undefined && plugins.data.length > 0 && (
        <Frame>
          {plugins.data.map((row) => (
            <PluginRow key={row.id} api={api} row={row} />
          ))}
        </Frame>
      )}
      {installing && (
        <>
          <SectionLabel>Install</SectionLabel>
          <InstallCard api={api} onDone={() => setInstalling(false)} />
        </>
      )}
      <p className="plugin-hint">
        A tool that acts on the world asks in the thread the first time, like any other;
        "Always allow" on that card is where a rule is made.
      </p>
    </div>
  )
}
