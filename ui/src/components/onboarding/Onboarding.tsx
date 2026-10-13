// The server names the steps. Each step stores its answers through the API.

import { useState } from 'react'

import type { ApiClient } from '../../api/client'
import {
  errorMessage,
  useAgents,
  useCompleteOnboarding,
  useOnboarding,
  useUser,
  useWorkspace,
} from '../../queries'

import { ComputerStep } from './ComputerStep'
import { ModelStep } from './ModelStep'
import { ProfileStep, type Profile } from './ProfileStep'
import { Stepper } from './Stepper'
import { WelcomeStep } from './WelcomeStep'
import type { StepId } from './steps'

import './onboarding.css'

export function Onboarding({ api }: { api: ApiClient }) {
  const status = useOnboarding(api)
  const agents = useAgents(api)
  const workspace = useWorkspace(api)
  const user = useUser(api)
  const complete = useCompleteOnboarding(api)
  const [selected, setStep] = useState<StepId | null>(null)
  const [profile, setProfile] = useState<Profile | null>(null)

  if (status.data === undefined || user.data === undefined) return null
  const steps = status.data.steps
  const current = selected ?? steps[0]
  const at = steps.indexOf(current)
  const sprite = agents.data?.find((agent) => agent.id === workspace.data?.chief_of_staff_agent_id)
  const finishError = complete.isError ? errorMessage(complete.error, 'Pagis could not finish setup.') : null
  const next = (answers = profile) => {
    if (at + 1 < steps.length) setStep(steps[at + 1])
    else complete.mutate({ userName: answers?.userName || undefined })
  }
  const back = () => setStep(steps[at - 1])

  return (
    <div className="onboarding">
      <div className="onboarding-titlebar" aria-hidden />
      <main className="onboarding-column" aria-label="Set up Pagis">
        <Stepper current={current} steps={steps} />
        {current === 'welcome' ? (
          <WelcomeStep api={api} sprite={sprite} onContinue={() => next()} />
        ) : current === 'model' ? (
          <ModelStep
            api={api}
            providers={status.data.providers}
            checks={status.data.checks}
            canSetProviderKeys={status.data.can_set_provider_keys}
            onBack={back}
            onContinue={() => next()}
            finishing={complete.isPending}
            finishError={finishError}
          />
        ) : current === 'profile' ? (
          <ProfileStep
            api={api}
            profile={profile}
            savedName={user.data.name ?? null}
            onBack={back}
            onContinue={(answers) => {
              setProfile(answers)
              next(answers)
            }}
            last={at === steps.length - 1}
            finishing={complete.isPending}
            finishError={finishError}
          />
        ) : current === 'computer' && status.data.docker !== null ? (
          <ComputerStep
            api={api}
            docker={status.data.docker}
            dockerEndpoint={status.data.docker_endpoint ?? null}
            canSetEndpoint={status.data.can_set_provider_keys}
            spriteId={sprite?.id}
            finishing={complete.isPending}
            finishError={finishError}
            onBack={back}
            onFinish={() => next()}
          />
        ) : null}
      </main>
    </div>
  )
}
