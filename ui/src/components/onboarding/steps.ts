// The server owns the offered order. The Product App supplies the labels.

import type { components } from '../../api/schema'

export type StepId = components['schemas']['OnboardingStep']

export const STEP_LABELS: Record<StepId, string> = {
  welcome: 'Welcome',
  model: 'Model',
  profile: 'Profile',
  computer: 'Computer',
}
