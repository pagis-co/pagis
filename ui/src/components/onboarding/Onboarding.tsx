// First run: welcome, providers, computer, then Pagis. The daemon holds
// the durable part — the stored keys, the Docker endpoint and the
// image job — so a reload or a daemon restart comes back where it left
// off, and only the step the user is reading lives here.

import { useState } from 'react'

import type { ApiClient } from '../../api/client'
import {
  errorMessage,
  useAgents,
  useCompleteOnboarding,
  useIsAdministrator,
  useOnboarding,
} from '../../queries'

import { ComputerStep } from './ComputerStep'
import { ModelStep } from './ModelStep'
import { Stepper } from './Stepper'
import { WelcomeStep } from './WelcomeStep'
import type { StepId } from './steps'

import './onboarding.css'

export function Onboarding({ api }: { api: ApiClient }) {
  const status = useOnboarding(api)
  const agents = useAgents(api)
  const isAdministrator = useIsAdministrator(api)
  const complete = useCompleteOnboarding(api)
  const [current, setStep] = useState<StepId>('welcome')

  if (status.data === undefined) return null
  const sprite = (agents.data ?? [])[0]

  return (
    <div className="onboarding">
      <div className="onboarding-titlebar" aria-hidden />
      <main className="onboarding-column" aria-label="Set up Pagis">
        <Stepper current={current} />
        {current === 'welcome' ? (
          <WelcomeStep
            sprite={sprite}
            onContinue={() => setStep('model')}
          />
        ) : current === 'model' ? (
          <ModelStep
            api={api}
            providers={status.data.providers}
            checks={status.data.checks}
            onBack={() => setStep('welcome')}
            onContinue={() => setStep('computer')}
          />
        ) : (
          <ComputerStep
            api={api}
            docker={status.data.docker}
            dockerEndpoint={status.data.docker_endpoint ?? null}
            canSetEndpoint={isAdministrator}
            spriteId={sprite?.id}
            finishing={complete.isPending}
            finishError={complete.isError ? errorMessage(complete.error, 'Pagis could not finish setup.') : null}
            onBack={() => setStep('model')}
            onFinish={() => complete.mutate({})}
          />
        )}
      </main>
    </div>
  )
}
