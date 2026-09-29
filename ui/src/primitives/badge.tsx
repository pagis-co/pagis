import type { ReactNode } from 'react'

import { cx } from './cx'
import './badge.css'

export type BadgeTone = 'neutral' | 'accent' | 'working' | 'waiting' | 'on-call' | 'failed'

export interface BadgeProps {
  tone?: BadgeTone
  children: ReactNode
  className?: string
  title?: string
}

/** A small state word on a soft fill. */
export function Badge({ tone = 'neutral', children, className, title }: BadgeProps) {
  return (
    <span className={cx('ui-badge', `ui-badge-${tone}`, className)} title={title}>
      {children}
    </span>
  )
}
