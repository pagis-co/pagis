import type { HTMLAttributes, ReactNode } from 'react'

import { cx } from './cx'
import './frame.css'

export interface FrameProps extends HTMLAttributes<HTMLDivElement> {
  /** One line of muted text under the frame, outside its border. */
  hint?: ReactNode
}

/** The bordered surface of a list: a 12 px radius, the border color
 * and the raised fill. The rows go inside. */
export function Frame({ hint, className, children, ...rest }: FrameProps) {
  const frame = (
    <div className={cx('ui-frame', className)} {...rest}>
      {children}
    </div>
  )
  if (hint === undefined) return frame
  return (
    <div className="ui-frame-with-hint">
      {frame}
      <p className="ui-frame-hint">{hint}</p>
    </div>
  )
}

export type RowProps = HTMLAttributes<HTMLDivElement>

/** One line of a Frame: 12 px by 16 px of padding and a 1 px divider
 * under every row but the last. */
export function Row({ className, ...rest }: RowProps) {
  return <div className={cx('ui-row', className)} {...rest} />
}
