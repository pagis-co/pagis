// The Software destination (ADR-0022): the workspace view of
// every package an Agent published. It is master-detail like
// Automations, and it is read-only: packages are written by Agents,
// and nothing about one needs the user, so there is no needs-you queue
// and no control.
//
// A list row carries the name, the latest Version, the author, the
// keywords, the tool count, and how many Contributions are still open.
// The detail carries the description, the tools, the Versions, the
// origin when the package is a Fork, and the Contributions with the
// patch collapsed.

import { useState } from 'react'
import { useIsMobile } from '../state/useIsMobile'
import { NavBar } from './phone/TopBar'
import { Package, X } from 'lucide-react'

import type { ApiClient } from '../api/client'
import { Avatar, Button, Frame, IconButton, Row, SectionLabel, Segmented } from '../primitives'
import {
  useAgents,
  useChannels,
  useContribution,
  useSoftware,
  useSoftwarePackage,
  usePlugins,
  usePlugin,
} from '../queries'
import { useComposerDraft } from '../state/composerDraft'
import { threadScope } from '../timeline'
import { AskAnAgent } from './AskAnAgent'
import { stateBadge } from './Plugins'

import './Automations.css'
import './Software.css'

/** What the list holds, in the words the user would use. */
const PACKAGES_EXPLANATION =
  'Small programs your sprites wrote, so the same job runs the same way again.'

function when(unixMs: number | null | undefined): string {
  if (unixMs === null || unixMs === undefined) return '—'
  return new Date(unixMs).toLocaleString()
}

/** One Contribution. The patch is one more call, so it loads only when
 *  the reader opens it. */
function ContributionRow({
  api,
  packageName,
  contribution,
}: {
  api: ApiClient
  packageName: string
  contribution: {
    id: string
    fork_package: string
    base_version: string
    fork_version: string
    summary: string
    status: string
    outcome_reason?: string | null
    created_at: number
  }
}) {
  const [open, setOpen] = useState(false)
  const record = useContribution(api, packageName, open ? contribution.id : null)

  return (
    <div className="software-contribution">
      <span className="automations-row-state">{contribution.status}</span>
      <strong>{contribution.summary}</strong>
      <span>
        {contribution.fork_package} {contribution.fork_version} against{' '}
        {packageName} {contribution.base_version}
      </span>
      <span>{when(contribution.created_at)}</span>
      {contribution.outcome_reason != null && (
        <span>{contribution.outcome_reason}</span>
      )}
      <Button
        size="sm"
        aria-expanded={open}
        onClick={() => setOpen((shown) => !shown)}
        className="automations-edit-open"
      >
        {open ? 'Hide patch' : 'Show patch'}
      </Button>
      {open &&
        (record.data === undefined ? (
          <p>Loading…</p>
        ) : (
          <pre className="software-patch">{record.data.patch}</pre>
        ))}
    </div>
  )
}

function PackageDetail({
  api,
  name,
  onBack,
}: {
  api: ApiClient
  name: string
  onBack: () => void
}) {
  const phone = useIsMobile()
  const [view, setView] = useState('tools')
  const held = useSoftwarePackage(api, name)
  const row = held.data

  return (
    <section className="automations-detail">
      {!phone && <Button className="automations-back" onClick={onBack}>
        Back to software
      </Button>}
      {row === undefined ? (
        <p>Loading…</p>
      ) : (
        <>
          <h3>{row.name}</h3>
          <dl className="automations-fields">
            <dt>Description</dt>
            <dd>{row.description}</dd>
            <dt>Author</dt>
            <dd>{row.author_name}</dd>
            <dt>Latest version</dt>
            <dd>{row.latest_version}</dd>
            <dt>Keywords</dt>
            <dd>{row.keywords.length === 0 ? '—' : row.keywords.join(', ')}</dd>
            <dt>Forked from</dt>
            <dd>
              {row.origin_package == null
                ? '—'
                : `${row.origin_package} ${row.origin_version ?? ''}`.trim()}
            </dd>
          </dl>
          {phone && <Segmented label="Package details" value={view} onValueChange={setView} items={[{ value: 'tools', label: 'Tools' }, { value: 'versions', label: 'Versions' }, { value: 'contributions', label: 'Contributions' }]} />}
          <section hidden={phone && view !== 'tools'} className="automations-history" aria-label="Tools">
            <h4>Tools</h4>
            {row.tools.length === 0 && <p>No tool.</p>}
            {row.tools.map((tool) => (
              <p key={tool.name}>
                {row.name}__{tool.name}: {tool.description}
              </p>
            ))}
          </section>
          <section hidden={phone && view !== 'versions'} className="automations-history" aria-label="Versions">
            <h4>Versions</h4>
            {row.versions.length === 0 && <p>No Version yet.</p>}
            {row.versions.map((version) => (
              <p key={version.version}>
                {version.version} — {when(version.published_at)}
                {version.notes === '' ? '' : ` — ${version.notes}`}
              </p>
            ))}
          </section>
          <section hidden={phone && view !== 'contributions'} className="automations-history" aria-label="Contributions">
            <h4>Contributions</h4>
            {row.contributions.length === 0 && <p>No Contribution yet.</p>}
            {row.contributions.map((contribution) => (
              <ContributionRow
                key={contribution.id}
                api={api}
                packageName={row.name}
                contribution={contribution}
              />
            ))}
          </section>
        </>
      )}
    </section>
  )
}

export function Software({
  api,
  onClose,
  onOpenChannel,
  selection,
  onSelect,
}: {
  api: ApiClient
  onClose: () => void
  /** Open one channel: the empty list sends the reader to a DM. */
  onOpenChannel: (channelId: string) => void
  selection?: string | null
  onSelect?: (name: string | null) => void
}) {
  const phone = useIsMobile()
  const [localSelected, setLocalSelected] = useState<string | null>(null)
  const selected = phone && selection !== undefined ? selection : localSelected
  const setSelected = (next: string | null) => phone && onSelect ? onSelect(next) : setLocalSelected(next)
  const packages = useSoftware(api)
  const agents = useAgents(api)
  const channels = useChannels(api)
  const setDraft = useComposerDraft((state) => state.set)

  if (phone) return <><NavBar back={{ label: selected ? 'Software' : 'You', onBack: selected ? () => setSelected(null) : onClose }} /><div className="phone-content phone-software">{selected ? <PackageDetail api={api} name={selected} onBack={() => setSelected(null)} /> : <><h1 className="phone-heading">Software</h1><p className="phone-hint">{PACKAGES_EXPLANATION}</p><section className="phone-section"><SectionLabel>Packages</SectionLabel><Frame>{(packages.data ?? []).map((held) => <Row key={held.name} chevron onClick={() => setSelected(held.name)}><span className="phone-icon-tile"><Package size={20} aria-hidden /></span><span className="phone-row-copy"><strong>{held.name}</strong><span className="phone-hint">{held.author_name} · {held.tool_count} tools · version {held.latest_version}{held.open_contributions ? ` · ${held.open_contributions} open contributions` : ''}</span></span></Row>)}</Frame>{packages.isError && <p role="alert" className="phone-hint">Could not read the packages.</p>}{packages.data?.length === 0 && <AskAnAgent kind="package" agents={agents.data ?? []} channels={channels.data ?? []} onOpenChannel={onOpenChannel} onDraft={(channelId, text) => setDraft(threadScope(channelId), text)} />}</section><PhonePlugins api={api} /></>}</div></>

  return (
    <div className="automations-panel">
      <header className="automations-header">
        <h2>Software</h2>
        <IconButton icon={X} label="Close software" variant="ghost" onClick={onClose} />
      </header>

      {selected === null ? (
        <section className="automations-section" aria-label="Packages">
          <h3>Packages</h3>
          <p className="automations-explanation">{PACKAGES_EXPLANATION}</p>
          {packages.data?.length === 0 && (
            <>
              <p>No package yet.</p>
              <AskAnAgent
                kind="package"
                agents={agents.data ?? []}
                channels={channels.data ?? []}
                onOpenChannel={onOpenChannel}
                onDraft={(channelId, text) =>
                  setDraft(threadScope(channelId), text)
                }
              />
            </>
          )}
          {(packages.data ?? []).map((held) => (
            <Button
              key={held.name}
              className="automations-row software-row"
              onClick={() => setSelected(held.name)}
            >
              <Avatar
                appearance={(agents.data ?? []).find((agent) => agent.id === held.author_agent_id)?.avatar} id={held.author_agent_id}
                name={held.author_name}
                size="sm"
              />
              <span>{held.name}</span>
              <span className="automations-row-state">{held.latest_version}</span>
              <span>{held.author_name}</span>
              <span>{held.keywords.join(', ')}</span>
              <span>
                {held.tool_count} tool{held.tool_count === 1 ? '' : 's'}
              </span>
              {held.open_contributions > 0 && (
                <span className="software-open-contributions">
                  {held.open_contributions} open contribution
                  {held.open_contributions === 1 ? '' : 's'}
                </span>
              )}
            </Button>
          ))}
        </section>
      ) : (
        <PackageDetail
          api={api}
          name={selected}
          onBack={() => setSelected(null)}
        />
      )}
    </div>
  )
}

function PhonePlugins({ api }: { api: ApiClient }) {
  const plugins = usePlugins(api)
  return <section className="phone-section"><SectionLabel>Plugins</SectionLabel><Frame>{(plugins.data ?? []).map((plugin) => <PhonePlugin key={plugin.id} api={api} id={plugin.id} name={plugin.name} version={plugin.manifest_version} />)}</Frame>{plugins.isPending && <p className="phone-hint">Reading the plugins…</p>}{plugins.isError && <p role="alert" className="phone-hint">Could not read the plugins.</p>}</section>
}
function PhonePlugin({ api, id, name, version }: { api: ApiClient; id: string; name: string; version: string }) {
  const plugin = usePlugin(api, id)
  return <Row><span className="phone-row-copy"><span>{name}</span><span className="phone-hint">Version {version}</span></span>{plugin.data && stateBadge(plugin.data)}{plugin.isError && <span role="alert" className="phone-hint">Could not read its state.</span>}</Row>
}
