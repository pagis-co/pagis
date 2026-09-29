import type { HTMLAttributes } from 'react'

import { cx } from './cx'
import './layout.css'

/** The four layout shells. Each one is a
 * landmark with its width from the tokens. The page puts the content
 * in; the shell draws only the box. */

export type ShellProps = HTMLAttributes<HTMLElement>

/** The 280 px sidebar: the product nav and the conversations. */
export function Sidebar({ className, ...rest }: ShellProps) {
  return <aside aria-label="Sidebar" className={cx('ui-sidebar', className)} {...rest} />
}

/** The 680 px reading column, centered in the space that is left. */
export function ReadingColumn({ className, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cx('ui-reading-column', className)} {...rest} />
}

/** The 220 px settings nav at the left of a settings page. */
export function SettingsNav({ className, ...rest }: ShellProps) {
  return <nav aria-label="Settings" className={cx('ui-settings-nav', className)} {...rest} />
}

export interface PanelProps extends ShellProps {
  /** Names the panel for a screen reader. */
  label: string
}

/** The 390 px panel at the right: replies, a desk, an inspector. */
export function Panel({ label, className, ...rest }: PanelProps) {
  return <aside aria-label={label} className={cx('ui-panel', className)} {...rest} />
}
