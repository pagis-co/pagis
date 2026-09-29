// The frame of one step: the content in the middle, which alone
// scrolls, and a footer that stays at the bottom of the window with
// Back on the left and the one primary action of the step on the right.
// The first step has no Back.

import type { ReactNode } from 'react'

import { Button } from '../../primitives'

export function StepLayout({
  onBack,
  action,
  children,
}: {
  onBack?: () => void
  /** The primary button of the step. */
  action: ReactNode
  children: ReactNode
}) {
  return (
    <>
      <div className="onboarding-body">
        <div className="onboarding-content">{children}</div>
      </div>
      <footer className="onboarding-actions">
        {onBack === undefined ? null : (
          <Button className="onboarding-back" onClick={onBack}>
            Back
          </Button>
        )}
        {action}
      </footer>
    </>
  )
}
