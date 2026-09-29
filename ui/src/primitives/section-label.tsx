import type { HTMLAttributes } from 'react'

import { cx } from './cx'
import './section-label.css'

export type SectionLabelProps = HTMLAttributes<HTMLDivElement>

/** The name over a group of rows or nav items: small, bold, spaced
 * upper case in the muted ink. */
export function SectionLabel({ className, ...rest }: SectionLabelProps) {
  return <div className={cx('ui-section-label', className)} {...rest} />
}
