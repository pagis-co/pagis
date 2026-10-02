// An on and off control with its label. The track and the knob
// read the tokens only.

import type { ReactNode } from 'react'

import { cx } from './cx'
import './switch.css'

export interface SwitchProps {
  checked: boolean
  onCheckedChange: (checked: boolean) => void
  children: ReactNode
  className?: string
  /** A switch that cannot change now, such as while a change runs. */
  disabled?: boolean
}

export function Switch({
  checked,
  onCheckedChange,
  children,
  className,
  disabled = false,
}: SwitchProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      className={cx('ui-switch', className)}
      onClick={() => onCheckedChange(!checked)}
    >
      <span className="ui-switch-track" aria-hidden="true" />
      {children}
    </button>
  )
}
