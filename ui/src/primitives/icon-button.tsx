import { forwardRef } from 'react'
import type { LucideIcon } from 'lucide-react'

import { Button } from './button'
import type { ButtonProps } from './button'
import { cx } from './cx'
import { Tooltip } from './tooltip'

export interface IconButtonProps extends Omit<ButtonProps, 'children'> {
  /** The Lucide icon to draw. */
  icon: LucideIcon
  /** Names the control for a screen reader and labels the tooltip. */
  label: string
}

/** A control that carries an icon and no words. The label is required:
 * without it the control has no accessible name. */
export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(
  function IconButton({ icon: Icon, label, className, size = 'md', ...rest }, ref) {
    const glyph = size === 'lg' ? 20 : size === 'sm' ? 14 : 16
    return (
      <Tooltip label={label}>
        <Button
          ref={ref}
          aria-label={label}
          size={size}
          className={cx('ui-icon-button', className)}
          {...rest}
        >
          <Icon size={glyph} aria-hidden focusable="false" />
        </Button>
      </Tooltip>
    )
  },
)
