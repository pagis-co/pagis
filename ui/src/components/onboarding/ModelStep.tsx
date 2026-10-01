// Step two: the providers. The person types a key for one provider or
// more. Each provider says what its key does in Pagis, and a summary
// says what the keys cover and which key a missing part needs. Continue
// stores each typed key, names the picked model as the whole default
// route, and moves on. With no pick, the daemon takes its preselection
// of the stored keys: the first model of the Model Preference that a
// keyed provider lists.
//
// A key check is the person's choice for each provider: it reads the
// provider's model list with the key, which generates nothing and costs
// nothing, and only a check that passed reads as ready. The daemon
// stores a checked key only when the provider answers, so a refused key
// never reads as held. The keys are held in this component while they
// are typed and never written to browser storage.

import { useState } from 'react'
import { CheckCircle2, Circle, Eye, EyeOff, XCircle } from 'lucide-react'

import type { ApiClient } from '../../api/client'
import { Button, Combobox, IconButton, Input } from '../../primitives'
import { providerName, providersOr } from '../../providers'
import {
  errorMessage,
  useCheckOnboardingProviderKey,
  useCheckProviderModel,
  useModelLists,
  useSetOnboardingDefaultModel,
  useSetOnboardingProviderKey,
} from '../../queries'

import { StepLayout } from './StepLayout'

/** What a provider key does, in the order the summary lists it. */
const USES: { id: string; name: string }[] = [
  { id: 'thinking', name: 'Thinking' },
  { id: 'spoken_replies', name: 'Spoken replies' },
  { id: 'dictation', name: 'Dictation' },
  { id: 'calls', name: 'Calls' },
]

/** Where a key the daemon already holds came from. */
const SOURCE_NOTES: Record<string, string> = {
  env: 'from the environment',
  config: 'from the config file',
  secret_file: 'in its secret file',
}

export interface ProviderStatus {
  provider: string
  configured: boolean
  source?: string | null
  uses: string[]
}

export interface KeyCheck {
  provider: string
  available: number
}

/** One provider's typed key, and the key the provider last refused. */
interface Draft {
  key: string
  refused: string | null
  failure: string | null
}

const EMPTY_DRAFT: Draft = { key: '', refused: null, failure: null }

/** "Thinking, spoken replies, dictation, calls". */
function usesLine(uses: string[]): string {
  const names = USES.filter((use) => uses.includes(use.id)).map((use) => use.name.toLowerCase())
  const line = names.join(', ')
  return line.charAt(0).toUpperCase() + line.slice(1)
}

export function ModelStep({
  api,
  providers,
  checks,
  onBack,
  onContinue,
}: {
  api: ApiClient
  providers: ProviderStatus[]
  checks: KeyCheck[]
  onBack: () => void
  onContinue: () => void
}) {
  const [drafts, setDrafts] = useState<Record<string, Draft>>({})
  const [picked, setPicked] = useState<string | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  const storeKey = useSetOnboardingProviderKey(api)
  const setDefaultModel = useSetOnboardingDefaultModel(api)

  const draftOf = (provider: string) => drafts[provider] ?? EMPTY_DRAFT
  const updateDraft = (provider: string, change: Partial<Draft>) =>
    setDrafts((current) => ({
      ...current,
      [provider]: { ...(current[provider] ?? EMPTY_DRAFT), ...change },
    }))
  // A typed key counts unless the provider just refused that same key.
  const typed = (provider: string) => {
    const draft = draftOf(provider)
    const key = draft.key.trim()
    return key !== '' && key !== draft.refused
  }
  const keyed = providers.filter((entry) => entry.configured || typed(entry.provider))
  const thinks = keyed.some((entry) => entry.uses.includes('thinking'))

  const lists = useModelLists(api, providers.some((entry) => entry.configured))
  const choices =
    lists.data?.providers.flatMap((entry) => entry.models.map((model) => model.candidate)) ?? []
  const model = picked ?? lists.data?.preselected ?? ''
  const busy = storeKey.isPending || setDefaultModel.isPending

  // Store each typed key, then name the picked model as the default
  // route. With no pick, the daemon takes its preselection.
  const finish = async () => {
    setFailure(null)
    for (const entry of providers) {
      if (!typed(entry.provider)) continue
      try {
        await storeKey.mutateAsync({
          provider: entry.provider,
          key: draftOf(entry.provider).key.trim(),
        })
        updateDraft(entry.provider, { key: '' })
      } catch (error) {
        setFailure(errorMessage(error, 'Cannot store the key. Check the daemon log.'))
        return
      }
    }
    try {
      await setDefaultModel.mutateAsync({ candidate: picked?.trim() || null })
      onContinue()
    } catch (error) {
      setFailure(errorMessage(error, 'Cannot save the model. Check the daemon log.'))
    }
  }

  return (
    <StepLayout
      onBack={onBack}
      action={
        <Button variant="primary" disabled={busy || !thinks} onClick={() => void finish()}>
          Continue
        </Button>
      }
    >
      <header className="onboarding-head">
        <h1>Connect your providers</h1>
        <p className="onboarding-lead">
          Use your own provider accounts and API keys. One key that thinks is enough to start,
          and each key you add gives Pagis more of what its provider does.
        </p>
      </header>
      <p className="onboarding-hint">
        Pagis seals each key in its secret file on this machine. Requests go straight to the
        provider, and the provider bills your account. A check is optional: it reads the
        provider's list of models, which costs nothing.
      </p>

      <div className="onboarding-provider-rows">
        {providers.map((entry) => (
          <ProviderRow
            key={entry.provider}
            api={api}
            entry={entry}
            available={
              checks.find((check) => check.provider === entry.provider)?.available ?? null
            }
            draft={draftOf(entry.provider)}
            onDraft={(change) => updateDraft(entry.provider, change)}
          />
        ))}
      </div>

      <ul className="onboarding-coverage" aria-label="What your keys cover">
        {USES.map((use) => {
          const serving = keyed
            .filter((entry) => entry.uses.includes(use.id))
            .map((entry) => providerName(entry.provider))
          const able = providers
            .filter((entry) => entry.uses.includes(use.id))
            .map((entry) => entry.provider)
          const covered = serving.length > 0
          return (
            <li key={use.id} className={covered ? 'onboarding-coverage-on' : undefined}>
              {covered ? <CheckCircle2 size={16} aria-hidden /> : <Circle size={16} aria-hidden />}
              <span className="onboarding-coverage-name">{use.name}</span>
              <span className="onboarding-coverage-line">
                {covered ? serving.join(', ') : `Needs a key for ${providersOr(able)}`}
              </span>
            </li>
          )
        })}
      </ul>

      {choices.length === 0 ? null : (
        <div className="onboarding-field">
          <span>Model</span>
          <Combobox
            label="Model"
            value={model}
            onValueChange={setPicked}
            items={choices}
            placeholder="provider/model"
          />
          <p className="onboarding-hint">
            Every sprite thinks on this model. Pagis selects it from the lists of your
            providers; pick another or type its id. Add fallback models later in Settings under
            Models.
          </p>
        </div>
      )}

      {failure === null ? null : (
        <p className="onboarding-result onboarding-result-failed" role="alert">
          <XCircle size={16} aria-hidden />
          {failure}
        </p>
      )}

      <p className="onboarding-note">
        Connect your Google account later in Settings, under Connections.
      </p>
    </StepLayout>
  )
}

/** One provider: what its key does, the key field and its check. */
function ProviderRow({
  api,
  entry,
  available,
  draft,
  onDraft,
}: {
  api: ApiClient
  entry: ProviderStatus
  /** The models the daemon's check of the held key counted. */
  available: number | null
  draft: Draft
  onDraft: (change: Partial<Draft>) => void
}) {
  const [shown, setShown] = useState(false)
  const check = useCheckProviderModel(api)
  const checkKey = useCheckOnboardingProviderKey(api)
  const name = providerName(entry.provider)
  const busy = check.isPending || checkKey.isPending
  const typed = draft.key.trim()

  // Ask the daemon to read the provider's model list with the typed
  // key, or with the held key when nothing is typed. The daemon stores
  // a typed key only when the provider answers. A failed check never
  // reads as ready.
  const test = async () => {
    onDraft({ failure: null })
    try {
      if (typed === '') {
        await check.mutateAsync(entry.provider)
      } else {
        await checkKey.mutateAsync({ provider: entry.provider, key: typed })
        onDraft({ key: '' })
      }
    } catch (error) {
      onDraft({
        failure: errorMessage(error, 'The provider did not answer.'),
        refused: typed === '' ? draft.refused : typed,
      })
    }
  }

  return (
    <div className="onboarding-provider-row" role="group" aria-label={name}>
      <div className="onboarding-provider-head">
        <span className="onboarding-provider-name">{name}</span>
        <span className="onboarding-provider-uses">{usesLine(entry.uses)}</span>
      </div>
      <span className="onboarding-field-row">
        <Input
          type={shown ? 'text' : 'password'}
          aria-label={`${name} API key`}
          placeholder={entry.configured ? 'Replace the stored key' : 'API key'}
          autoComplete="off"
          value={draft.key}
          onChange={(event) => onDraft({ key: event.target.value })}
        />
        <IconButton
          icon={shown ? EyeOff : Eye}
          label={shown ? `Hide the ${name} key` : `Show the ${name} key`}
          onClick={() => setShown((was) => !was)}
        />
        <Button onClick={() => void test()} disabled={busy || (!entry.configured && typed === '')}>
          {busy ? 'Testing…' : 'Test connection'}
        </Button>
      </span>
      {entry.configured && typed === '' && available === null ? (
        <p className="onboarding-hint">
          Pagis already holds a key {SOURCE_NOTES[entry.source ?? ''] ?? 'for this provider'}.
        </p>
      ) : null}
      {available === null || typed !== '' ? null : (
        <p className="onboarding-result onboarding-result-ok" role="status">
          <CheckCircle2 size={16} aria-hidden />
          The key works; {available} {available === 1 ? 'model' : 'models'} available
        </p>
      )}
      {draft.failure === null ? null : (
        <p className="onboarding-result onboarding-result-failed" role="alert">
          <XCircle size={16} aria-hidden />
          {draft.failure}
        </p>
      )}
    </div>
  )
}
