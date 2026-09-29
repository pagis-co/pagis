import type { SVGAttributes } from 'react'

import { cx } from './cx'
import './logo-mark.css'

export type LogoMarkProps = Omit<SVGAttributes<SVGSVGElement>, 'children'>

/** The Pagis mark: three desks that make a P, with the fourth place
 * open. It is one em square, so it takes the size of the name beside
 * it, and it is hidden from assistive technology because that name
 * says the same thing. `assets/brand` holds the same drawing as files. */
export function LogoMark({ className, ...rest }: LogoMarkProps) {
  return (
    <svg
      className={cx('ui-logo-mark', className)}
      viewBox="20 20 120 120"
      aria-hidden="true"
      focusable="false"
      {...rest}
    >
      <rect className="ui-logo-mark-top" x="20" y="20" width="56" height="56" rx="10" />
      <path
        className="ui-logo-mark-bowl"
        d="M94 20H112A28 28 0 0 1 112 76H94A10 10 0 0 1 84 66V30A10 10 0 0 1 94 20Z"
      />
      <rect className="ui-logo-mark-bottom" x="20" y="84" width="56" height="56" rx="10" />
    </svg>
  )
}
