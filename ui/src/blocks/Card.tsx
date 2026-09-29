// The one frame of every timeline block: the Frame primitive
// with one radius, a header line that names the act and the place, a
// body, and a footer that holds at most one primary action. A block
// that settles collapses to one SettledLine inside the same frame.

import type { HTMLAttributes, ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'

import { Frame, cx } from '../primitives'
import type { FrameProps } from '../primitives'

import './card.css'

export function Card({ className, ...rest }: FrameProps) {
  return <Frame className={cx('block-card', className)} {...rest} />
}

export type SettledTone = 'working' | 'failed' | 'waiting' | 'neutral'

export function CardHeader({
  icon: Icon,
  act,
  place,
  className,
  children,
  ...rest
}: {
  icon?: LucideIcon
  /** What the block does, in bold. */
  act: ReactNode
  /** Where, or with whom, in the muted ink. */
  place?: ReactNode
} & HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={cx('block-card-header', className)} {...rest}>
      {Icon !== undefined && <Icon size={16} aria-hidden focusable="false" />}
      <span className="block-card-title">
        <strong>{act}</strong>
        {place !== undefined && place !== null && (
          <span className="block-card-place">{place}</span>
        )}
      </span>
      {children !== undefined && (
        <span className="block-card-trailing">{children}</span>
      )}
    </div>
  )
}

export function CardBody({ className, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cx('block-card-body', className)} {...rest} />
}

export function CardFooter({ className, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cx('block-card-footer', className)} {...rest} />
}

/** The one line a settled block keeps: a dot in the state hue, what
 *  it was, and the decision with its clock at the end. */
export function SettledLine({
  tone,
  aside,
  className,
  children,
  ...rest
}: {
  tone: SettledTone
  aside?: ReactNode
} & HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cx('block-settled', `block-settled-${tone}`, className)}
      {...rest}
    >
      <span className="block-settled-dot" aria-hidden />
      <span className="block-settled-text">{children}</span>
      {aside !== undefined && <span className="block-settled-aside">{aside}</span>}
    </div>
  )
}
