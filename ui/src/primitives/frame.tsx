import type { HTMLAttributes, ReactNode } from 'react'
import { Link, type LinkProps } from '@tanstack/react-router'
import { ChevronRight } from 'lucide-react'

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

export interface RowProps extends HTMLAttributes<HTMLElement> {
  roomy?: boolean
  value?: ReactNode
  hint?: ReactNode
  chevron?: boolean
  /** A place of this app. The row is a router link to it. */
  href?: string
  /** Turns off the action of a row with `onClick` or `href`. */
  disabled?: boolean
}

/** One line of a Frame: 12 px by 16 px of padding and a 1 px divider
 * under every row but the last. */
export function Row({ className, children, value, hint, chevron, href, onClick, roomy, disabled, ...rest }: RowProps) {
  const content = <>{hint === undefined ? children : <span className="ui-row-copy"><span>{children}</span><span className="ui-row-hint">{hint}</span></span>}{value !== undefined && <span className="ui-row-value">{value}</span>}{chevron && <ChevronRight className="ui-row-chevron" size={16} aria-hidden />}</>
  const classes = cx('ui-row', roomy && 'ui-row-roomy', (href || onClick) && 'ui-row-interactive', className)
  // The router types its links by route path and asks for `to`; it also
  // takes a built `href`, which it parses into the same location.
  if (href) return <Link {...({ href } as LinkProps)} className={classes} disabled={disabled} onClick={onClick} {...rest}>{content}</Link>
  if (onClick) return <button type="button" className={classes} disabled={disabled} onClick={onClick} {...rest}>{content}</button>
  return <div className={classes} {...rest}>{content}</div>
}
