// The steps across the top of onboarding. It is a
// signpost, not a control: a step is never clickable, because a later
// step needs what an earlier one recorded.

import { Check } from 'lucide-react'

import { cx } from '../../primitives'

import { STEP_LABELS, type StepId } from './steps'

export function Stepper({ current, steps }: { current: StepId; steps: StepId[] }) {
  const at = steps.indexOf(current)
  return (
    <ol className="onboarding-stepper" aria-label="Setup steps">
      {steps.map((step, index) => {
        const done = index < at
        return (
          <li
            key={step}
            className={cx(
              'onboarding-step',
              done && 'onboarding-step-done',
              index === at && 'onboarding-step-current',
            )}
            aria-current={index === at ? 'step' : undefined}
          >
            <span className="onboarding-step-mark" aria-hidden>
              {done ? <Check size={14} /> : index + 1}
            </span>
            <span className="onboarding-step-label">{STEP_LABELS[step]}</span>
          </li>
        )
      })}
    </ol>
  )
}
