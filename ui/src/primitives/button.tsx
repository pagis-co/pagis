import { forwardRef } from 'react'
import type { ButtonHTMLAttributes } from 'react'

import { cx } from './cx'
import './button.css'

export type ButtonVariant =
  | 'primary'
  | 'outline'
  | 'ghost'
  | 'danger'
  | 'danger-quiet'
  | 'link'
export type ButtonSize = 'sm' | 'md' | 'lg' | 'xl'
export type ButtonShape = 'default' | 'pill' | 'row'

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant
  size?: ButtonSize
  /** `pill` rounds the ends: the shape of a chip and of a filter.
   * `row` fills one row of a Frame: the full width, the row padding,
   * square corners, and a divider under it when a row follows. */
  shape?: ButtonShape
}

/** The one button of the product. `type` defaults to `button`, so a
 * button in a form submits only when it asks to. */
export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = 'outline', size = 'md', shape = 'default', className, type, ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type ?? 'button'}
      className={cx(
        'ui-button',
        `ui-button-${variant}`,
        `ui-button-${size}`,
        shape !== 'default' && `ui-button-${shape}`,
        className,
      )}
      {...rest}
    />
  )
})
