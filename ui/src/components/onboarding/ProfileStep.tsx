// Profile records the Person's name and the Workspace timezone.

import { useState } from 'react'

import type { ApiClient } from '../../api/client'
import { Button, Input, Select } from '../../primitives'
import { errorMessage, useSetTimezone, useWorkspace } from '../../queries'
import { deviceTimezone, knownTimezones } from '../../timezone'

import { StepLayout } from './StepLayout'

export interface Profile {
  userName: string
  timezone: string
}

export function ProfileStep({
  api,
  profile,
  savedName,
  onBack,
  onContinue,
  last,
  finishing,
  finishError,
}: {
  api: ApiClient
  profile: Profile | null
  savedName: string | null
  onBack: () => void
  onContinue: (profile: Profile) => void
  last: boolean
  finishing: boolean
  finishError: string | null
}) {
  const workspace = useWorkspace(api)
  const [name, setName] = useState<string | null>(profile?.userName ?? null)
  const [timezone, setTimezone] = useState(profile?.timezone ?? deviceTimezone())
  const save = useSetTimezone(api)
  const userName = name ?? savedName ?? ''
  const zone = timezone ?? workspace.data?.timezone ?? ''
  const next = async () => {
    try {
      await save.mutateAsync(zone)
      onContinue({ userName: userName.trim(), timezone: zone })
    } catch {
      // The mutation keeps the error and the form keeps the answers.
    }
  }
  return (
    <StepLayout
      onBack={onBack}
      action={
        <Button
          variant="primary"
          disabled={save.isPending || finishing || !zone}
          onClick={() => void next()}
        >
          {last ? 'Continue to Pagis' : 'Continue'}
        </Button>
      }
    >
      <header className="onboarding-head">
        <h1>Your profile</h1>
        <p className="onboarding-lead">Tell your sprites what to call you and which clock to use.</p>
      </header>
      <label className="onboarding-field">
        Your name
        <Input
          aria-label="Your name"
          autoComplete="name"
          value={userName}
          onChange={(event) => setName(event.target.value)}
        />
      </label>
      <div className="onboarding-field">
        <span>Timezone</span>
        <Select
          label="Timezone"
          value={zone}
          onValueChange={setTimezone}
          items={knownTimezones(zone).map((value) => ({ value, label: value }))}
        />
        <p className="onboarding-hint">Your Schedules and Daily report use the Workspace timezone.</p>
      </div>
      {(save.isError || finishError !== null) && (
        <p className="onboarding-error" role="alert">
          {save.isError ? errorMessage(save.error, 'Cannot save the timezone.') : finishError}
        </p>
      )}
    </StepLayout>
  )
}
