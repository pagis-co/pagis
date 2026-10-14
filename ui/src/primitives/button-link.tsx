import type { AnchorHTMLAttributes } from 'react'
import type { ButtonShape, ButtonSize, ButtonVariant } from './button'
import { cx } from './cx'
import './button.css'

export interface ButtonLinkProps extends AnchorHTMLAttributes<HTMLAnchorElement> {
  variant?: ButtonVariant
  size?: ButtonSize
  shape?: ButtonShape
}

/** A native link with the button styles. */
export function ButtonLink({
  variant = 'outline',
  size = 'md',
  shape = 'default',
  className,
  ...rest
}: ButtonLinkProps) {
  return (
    <a
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
}
