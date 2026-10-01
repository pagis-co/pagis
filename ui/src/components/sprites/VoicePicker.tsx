// The sprite voice: one of the voices of the model that speaks for the
// Workspace (the Provider Voice List, ADR-0020). An empty value is the
// model's default voice, its first. A voice the sprite holds and the
// model does not offer stays, and a reply speaks in the default.

import type { ApiClient } from '../../api/client'
import { Select } from '../../primitives'
import { useVoices } from '../../queries'

const DEFAULT_VOICE = 'default-voice'

export function VoicePicker({
  api,
  value,
  onChange,
}: {
  api: ApiClient
  /** The sprite's voice, or `''` for the model's default. */
  value: string
  onChange: (voice: string) => void
}) {
  const voices = useVoices(api)
  const page = voices.data
  const offered = (page?.items ?? []).map((voice) => ({
    value: voice.id,
    label: voice.name ?? voice.id,
  }))
  const speaks = page?.provider != null
  const defaultLabel = !speaks
    ? 'No key serves spoken replies'
    : offered.length > 0
      ? `Default voice (${offered[0].label})`
      : 'Default voice'
  const held =
    value !== '' && !offered.some((voice) => voice.value === value)
      ? [{ value, label: `${value} (not a voice of ${page?.model ?? 'the model that speaks'})` }]
      : []

  return (
    <Select
      label="Sprite voice"
      value={value === '' ? DEFAULT_VOICE : value}
      onValueChange={(next) => onChange(next === DEFAULT_VOICE ? '' : next)}
      disabled={!speaks}
      items={[
        { value: DEFAULT_VOICE, label: defaultLabel },
        ...offered,
        ...held,
      ]}
    />
  )
}
