// The route a first run walks: welcome, model, computer. The shell
// renders one step at a time and the stepper reads this list, so the
// order is written once.

export type StepId = 'welcome' | 'model' | 'computer'

export interface Step {
  id: StepId
  label: string
}

export const STEPS: Step[] = [
  { id: 'welcome', label: 'Welcome' },
  { id: 'model', label: 'Model' },
  { id: 'computer', label: 'Computer' },
]

export function stepIndex(step: StepId): number {
  return STEPS.findIndex((entry) => entry.id === step)
}
