import { Plus } from 'lucide-react'
import { PhoneHeaderAction } from './phone/TopBar'
// The Models section: every Agent thinks on an alias. The candidate
// field offers the models each provider lists for the installation's
// key, and it still takes a typed `provider/model` id.

import { useState } from 'react'

import type { ApiClient, ModelAliasDto } from '../api/client'
import { Badge, IconButton, Button, Combobox, Dialog, Frame, Input, Row, SectionLabel, Sheet, Textarea } from '../primitives'
import { useIsMobile } from '../state/useIsMobile'
import { providersOr } from '../providers'
import {
  errorMessage,
  useCreateModelAlias,
  useDeleteModelAlias,
  useModelAliases,
  useModelLists,
  useUpdateModelAlias,
} from '../queries'

import './ModelsSettings.css'

/** What the product runs on each alias it seeds. An alias the user
 *  adds carries no note. */
const ALIAS_NOTES: Record<string, string> = {
  default: 'every seeded sprite',
  transcribe: 'buffered speech to text',
  speak: 'the sprite voice',
  phone: 'telephone conversations',
  'phone-classifier': 'outbound answer detection',
  reflect: 'memory reflection Runs',
}

/** The providers an alias's candidates name, any one of which would
 *  make it reachable. */
function neededProviders(candidates: string[]): string {
  return providersOr(candidates.map((candidate) => candidate.split('/')[0]))
}

function candidatesOf(value: string): string[] {
  return value
    .split('\n')
    .map((candidate) => candidate.trim())
    .filter(Boolean)
}

/** The modal that names an alias and lists its candidates in order.
 *  With an alias it edits that one; without, it adds a new one. */
function AliasDialog({
  api,
  alias,
  onClose,
}: {
  api: ApiClient
  alias: ModelAliasDto | null
  onClose: () => void
}) {
  const Modal = useIsMobile() ? Sheet : Dialog
  const [name, setName] = useState('')
  const [draft, setDraft] = useState(alias?.candidates.join('\n') ?? '')
  const [settingDrafts, setSettingDrafts] = useState<Record<string, string>>(() =>
    Object.fromEntries(
      (alias?.settings ?? []).map((setting) => [setting.alias, setting.candidates.join('\n')]),
    ),
  )
  const [adding, setAdding] = useState('')
  const create = useCreateModelAlias(api)
  const update = useUpdateModelAlias(api)
  const remove = useDeleteModelAlias(api)
  const lists = useModelLists(api)
  const listed = (lists.data?.providers ?? []).flatMap((entry) => entry.models.map((model) => model.candidate))
  const candidates = candidatesOf(draft)
  const activeSettings = (alias?.settings ?? []).filter((setting) =>
    setting.when_candidates.some((candidate) => candidates.includes(candidate)),
  )
  const pending = create.isPending || update.isPending || remove.isPending
  const error = create.error ?? update.error ?? remove.error
  const ready = candidates.length > 0
    && activeSettings.every((setting) => candidatesOf(settingDrafts[setting.alias] ?? '').length > 0)
    && (alias !== null || name.trim() !== '')

  // Append the chosen or typed candidate to the ordered list.
  function addCandidate() {
    const candidate = adding.trim()
    if (candidate === '') return
    setDraft([...candidates, candidate].join('\n'))
    setAdding('')
  }

  async function save() {
    if (alias === null) {
      create.mutate({ alias: name.trim(), candidates }, { onSuccess: onClose })
    } else {
      try {
        await update.mutateAsync({ alias: alias.alias, candidates })
        for (const setting of activeSettings) {
          await update.mutateAsync({
            alias: setting.alias,
            candidates: candidatesOf(settingDrafts[setting.alias] ?? ''),
          })
        }
        onClose()
      } catch {
        // The mutation error is shown below the fields.
      }
    }
  }

  return (
    <Modal
      open
      onOpenChange={(open) => { if (!open) onClose() }}
      title={alias === null ? 'Add an alias' : `Edit ${alias.alias}`}
      description="The first candidate that answers wins."
      footer={
        <>
          {alias !== null && (
            <Button
              variant="danger"
              disabled={pending}
              onClick={() => remove.mutate(alias.alias, { onSuccess: onClose })}
            >
              Delete
            </Button>
          )}
          <Button variant="primary" disabled={pending || !ready} onClick={() => { void save() }}>
            {alias === null ? 'Add' : 'Save'}
          </Button>
        </>
      }
    >
      <div className="models-dialog-fields">
        {alias === null && (
          <label className="models-field">
            <span>Name</span>
            <Input value={name} onChange={(event) => setName(event.target.value)} />
          </label>
        )}
        <label className="models-field">
          <span>Candidates, one per line</span>
          <Textarea
            rows={4}
            placeholder="provider/model"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
        </label>
        <div className="models-field">
          <span>Add a candidate</span>
          <span className="models-add">
            <Combobox
              label="Add a candidate"
              placeholder="Choose a listed model or type provider/model"
              value={adding}
              onValueChange={setAdding}
              items={listed}
              onKeyDown={(event) => {
                if (event.key === 'Enter') {
                  event.preventDefault()
                  addCandidate()
                }
              }}
            />
            <Button disabled={adding.trim() === ''} onClick={addCandidate}>Add candidate</Button>
          </span>
        </div>
        <p className="models-note">OpenRouter candidates use openrouter/provider/model.</p>
        <p className="models-note">
          The choices are the models each provider lists for the installation&apos;s key.
        </p>
        {activeSettings.map((setting) => (
          <label className="models-field" key={setting.alias}>
            <span>{setting.label}</span>
            <Textarea
              aria-label={setting.label}
              rows={2}
              placeholder="provider/model"
              value={settingDrafts[setting.alias] ?? ''}
              onChange={(event) => setSettingDrafts((current) => ({
                ...current,
                [setting.alias]: event.target.value,
              }))}
            />
            <span className="models-note">{setting.description}</span>
          </label>
        ))}
        {error !== null && <p className="models-error">{errorMessage(error, 'The change did not save.')}</p>}
      </div>
    </Modal>
  )
}

/** The Models section: a person's own aliases. The provider keys
 *  belong to the Org, so the Administration Interface holds them. */
export function ModelsSettings({ api }: { api: ApiClient }) {
  const phone = useIsMobile()
  const aliases = useModelAliases(api)
  const [aliasDialog, setAliasDialog] = useState<'closed' | 'new' | ModelAliasDto>('closed')
  const nestedAliases = new Set(
    (aliases.data ?? []).flatMap((alias) => alias.settings.map((setting) => setting.alias)),
  )

  return (
    <section className="models-settings">
      <div className="models-title">
        <h3>Models</h3>
        <span className="models-lead">
          Every sprite thinks on an alias. An alias names its candidates in order;
          the first that answers wins.
        </span>
        {phone ? <PhoneHeaderAction><IconButton icon={Plus} label="Add an alias" variant="link" onClick={() => setAliasDialog('new')} /></PhoneHeaderAction> : <Button variant="primary" className="models-title-action" onClick={() => setAliasDialog('new')}>Add an alias</Button>}
      </div>

      <SectionLabel>Aliases</SectionLabel>
      <Frame>
        {(aliases.data ?? []).filter((alias) => phone || !nestedAliases.has(alias.alias)).map((alias) => (
          phone ? <Row key={alias.alias} chevron onClick={() => setAliasDialog(alias)}><span className="phone-row-copy"><code>{alias.alias}</code>{!alias.reachable && <span><Badge tone="waiting">Needs a key for {neededProviders(alias.candidates)}</Badge></span>}<span className="phone-hint">{[ALIAS_NOTES[alias.alias], alias.candidates[0]].filter(Boolean).join(' · ')}</span></span></Row> : <Row key={alias.alias}>
            <span className="models-alias-name">{alias.alias}</span>
            <ul className="models-chips" aria-label={`Candidates of ${alias.alias}`}>
              {alias.candidates.map((candidate, index) => (
                <li key={candidate} className="models-chip">
                  {index + 1} · {candidate}
                </li>
              ))}
            </ul>
            {alias.alias in ALIAS_NOTES && (
              <span className="models-note">{ALIAS_NOTES[alias.alias]}</span>
            )}
            {alias.reachable ? null : (
              <Badge tone="waiting">Needs a key for {neededProviders(alias.candidates)}</Badge>
            )}
            <span className="models-spacer" />
            <Button aria-label={`Edit ${alias.alias}`} onClick={() => setAliasDialog(alias)}>
              Edit
            </Button>
          </Row>
        ))}
      </Frame>

      {aliasDialog !== 'closed' && (
        <AliasDialog
          api={api}
          alias={aliasDialog === 'new' ? null : aliasDialog}
          onClose={() => setAliasDialog('closed')}
        />
      )}
    </section>
  )
}
