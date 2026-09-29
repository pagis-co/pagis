import type { ReactNode } from 'react'
import * as DropdownMenu from '@radix-ui/react-dropdown-menu'

import './menu.css'

export interface MenuItem {
  label: string
  onSelect: () => void
  disabled?: boolean
  /** A destructive item is drawn in the failed hue. */
  danger?: boolean
}

export interface MenuProps {
  /** The control that opens the menu. It must accept a ref. */
  trigger: ReactNode
  items: MenuItem[]
  label?: string
}

/** A menu on the Radix DropdownMenu: arrow keys move, Enter chooses,
 * Escape closes and returns the focus to the trigger. */
export function Menu({ trigger, items, label }: MenuProps) {
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>{trigger}</DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className="ui-menu" aria-label={label} sideOffset={4} align="start">
          {items.map((item) => (
            <DropdownMenu.Item
              key={item.label}
              disabled={item.disabled}
              onSelect={item.onSelect}
              className={item.danger ? 'ui-menu-item ui-menu-item-danger' : 'ui-menu-item'}
            >
              {item.label}
            </DropdownMenu.Item>
          ))}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  )
}
