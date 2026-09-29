import type { ReactNode } from 'react'
import * as RadixTabs from '@radix-ui/react-tabs'

import { cx } from './cx'
import './tabs.css'

export interface TabItem {
  value: string
  label: ReactNode
}

export interface TabsProps {
  value: string
  onValueChange: (value: string) => void
  items: TabItem[]
  /** Names the tab list for a screen reader. */
  label: string
  className?: string
  children?: ReactNode
}

/** A tab strip on the Radix Tabs: the arrow keys, Home and End move the
 * selection. The caller draws the panel of the selected tab as the
 * children, so only one panel is ever mounted. */
export function Tabs({
  value,
  onValueChange,
  items,
  label,
  className,
  children,
}: TabsProps) {
  return (
    <RadixTabs.Root
      value={value}
      onValueChange={onValueChange}
      className={cx('ui-tabs', className)}
    >
      <RadixTabs.List aria-label={label} className="ui-tabs-list">
        {items.map((item) => (
          <RadixTabs.Trigger key={item.value} value={item.value} className="ui-tab">
            {item.label}
          </RadixTabs.Trigger>
        ))}
      </RadixTabs.List>
      <RadixTabs.Content value={value} className="ui-tabs-content">
        {children}
      </RadixTabs.Content>
    </RadixTabs.Root>
  )
}
