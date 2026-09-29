// Step two: one provider, one key and one model. Continue stores the
// key, names the picked model as the whole default route, and moves on.
// The connection check is the person's choice: it reads the provider's
// model list with the key, which generates nothing and costs nothing,
// and only a check that passed reads as ready. The daemon stores a
// checked key only when the provider answers, so a refused key never
// reads as held. The picker offers the
// provider's list, newest first, with the newest preselected. The key is
// held in this component while it is typed and never written to browser
// storage.

import { useState } from 'react'
import { CheckCircle2, Eye, EyeOff, XCircle } from 'lucide-react'

import type { ApiClient } from '../../api/client'
import { Button, IconButton, Input, Select, cx } from '../../primitives'
import {
  errorMessage,
  useCheckOnboardingProviderKey,
  useCheckProviderModel,
  useModelLists,
  useSetOnboardingDefaultModel,
  useSetOnboardingProviderKey,
} from '../../queries'

import { StepLayout } from './StepLayout'

const PROVIDER_NAMES: Record<string, string> = {
  anthropic: 'Anthropic',
  openai: 'OpenAI',
  openrouter: 'OpenRouter',
}

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
}

export function ModelStep({
  api,
  providers,
  verified,
  onBack,
  onContinue,
}: {
  api: ApiClient
  providers: ProviderStatus[]
  verified?: { provider: string; available: number } | null
  onBack: () => void
  onContinue: () => void
}) {
  const [provider, setProvider] = useState(verified?.provider ?? providers[0]?.provider ?? 'anthropic')
  const [key, setKey] = useState('')
  const [shown, setShown] = useState(false)
  const [checked, setChecked] = useState<number | null>(
    verified?.provider === provider ? verified.available : null,
  )
  const [failure, setFailure] = useState<string | null>(null)
  // The typed key the provider just refused. Continue does not store it.
  const [refused, setRefused] = useState<string | null>(null)
  const [picked, setPicked] = useState<string | null>(null)
  const storeKey = useSetOnboardingProviderKey(api)
  const check = useCheckProviderModel(api)
  const checkKey = useCheckOnboardingProviderKey(api)
  const setDefaultModel = useSetOnboardingDefaultModel(api)

  const selected = providers.find((entry) => entry.provider === provider)
  const held = selected?.configured === true
  const busy =
    storeKey.isPending || check.isPending || checkKey.isPending || setDefaultModel.isPending
  // A check that passed proves a key is held, whatever the list says.
  const hasKey = held || checked !== null
  const lists = useModelLists(api, hasKey)
  const prefix = `${provider}/`
  const listed = (lists.data ?? []).find((entry) => entry.provider === provider)
  const choices = listed?.models.map((model) => model.candidate.slice(prefix.length)) ?? []
  // The daemon names the preselection: the newest listed chat model.
  const model = picked ?? listed?.preselected ?? choices[0] ?? null

  const pickProvider = (next: string) => {
    setProvider(next)
    setKey('')
    setChecked(null)
    setFailure(null)
    setPicked(null)
  }

  // Store the typed key, if there is one. Answer whether a key is held.
  const store = async (): Promise<boolean> => {
    if (key.trim() === '') return hasKey
    try {
      await storeKey.mutateAsync({ provider, key: key.trim() })
      setKey('')
      return true
    } catch (error) {
      setFailure(
        errorMessage(error, 'Cannot store the key. Check the daemon log.'),
      )
      return false
    }
  }

  // Ask the daemon to read the provider's model list with the typed
  // key, or with the held key when nothing is typed. The daemon stores
  // a typed key only when the provider answers. A failed check never
  // reads as ready.
  const test = async () => {
    setChecked(null)
    setFailure(null)
    const typed = key.trim()
    try {
      const result =
        typed === ''
          ? await check.mutateAsync(provider)
          : await checkKey.mutateAsync({ provider, key: typed })
      if (typed !== '') setKey('')
      setChecked(result.available)
    } catch (error) {
      if (typed !== '') setRefused(typed)
      setFailure(errorMessage(error, 'The provider did not answer.'))
    }
  }

  // Store the key, then name the picked model as the default route.
  // With no list to pick from, the daemon takes its preselection.
  const finish = async () => {
    if (!(await store())) return
    try {
      await setDefaultModel.mutateAsync({ provider, model })
      onContinue()
    } catch (error) {
      setFailure(errorMessage(error, 'Cannot save the model. Check the daemon log.'))
    }
  }

  return (
    <StepLayout
      onBack={onBack}
      action={
        <Button
          variant="primary"
          disabled={
            busy || (!hasKey && key.trim() === '') || (refused !== null && key.trim() === refused)
          }
          onClick={() => void finish()}
        >
          Continue
        </Button>
      }
    >
      <header className="onboarding-head">
        <h1>Connect your model</h1>
        <p className="onboarding-lead">
          Use your own provider account and API key.
        </p>
      </header>

      <div
        className="onboarding-providers"
        role="radiogroup"
        aria-label="Model provider"
      >
        {providers.map((entry) => (
          <label
            key={entry.provider}
            className={cx(
              'onboarding-provider',
              entry.provider === provider && 'onboarding-provider-selected',
            )}
          >
            <input
              type="radio"
              name="model-provider"
              value={entry.provider}
              checked={entry.provider === provider}
              onChange={() => pickProvider(entry.provider)}
            />
            <span>{PROVIDER_NAMES[entry.provider] ?? entry.provider}</span>
          </label>
        ))}
      </div>

      <label className="onboarding-field">
        <span>API key</span>
        <span className="onboarding-field-row">
          <Input
            type={shown ? 'text' : 'password'}
            placeholder={held ? 'Replace the stored key' : 'API key'}
            autoComplete="off"
            value={key}
            onChange={(event) => {
              setKey(event.target.value)
              setChecked(null)
            }}
          />
          <IconButton
            icon={shown ? EyeOff : Eye}
            label={shown ? 'Hide the key' : 'Show the key'}
            onClick={() => setShown((was) => !was)}
          />
        </span>
      </label>
      <p className="onboarding-hint">
        {held
          ? `Pagis already holds a key ${SOURCE_NOTES[selected?.source ?? ''] ?? 'for this provider'}. `
          : 'Pagis seals the key in its secret file on this machine. '}
        Model requests go straight to the provider you choose, and the provider
        bills your account. The check is optional: it reads the provider's list
        of models, which costs nothing.
      </p>

      <div className="onboarding-check">
        <Button onClick={() => void test()} disabled={busy || (!held && key.trim() === '')}>
          {busy ? 'Testing…' : 'Test connection'}
        </Button>
        {checked === null ? null : (
          <p className="onboarding-result onboarding-result-ok" role="status">
            <CheckCircle2 size={16} aria-hidden />
            The key works; {checked} {checked === 1 ? 'model' : 'models'} available
          </p>
        )}
        {failure === null ? null : (
          <p className="onboarding-result onboarding-result-failed" role="alert">
            <XCircle size={16} aria-hidden />
            {failure}
          </p>
        )}
      </div>

      {choices.length === 0 ? null : (
        <div className="onboarding-field">
          <span>Model</span>
          <Select
            label="Model"
            value={model ?? ''}
            onValueChange={setPicked}
            items={choices.map((choice) => ({ value: choice, label: choice }))}
          />
        </div>
      )}
      {choices.length === 0 ? null : (
        <p className="onboarding-hint">
          Every sprite thinks on this model. The list is the provider's, newest
          first. Add fallback models later in Settings under Models.
        </p>
      )}

      <p className="onboarding-note">
        Connect your Google account later in Settings, under Connections.
      </p>
    </StepLayout>
  )
}
