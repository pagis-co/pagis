import { Check } from 'lucide-react'
import type { ReactNode } from 'react'
import * as DropdownMenu from '@radix-ui/react-dropdown-menu'

import './menu.css'

export type MenuItem = {
  label: string
  detail?: string
  current?: boolean
  disabled?: boolean
  /** A destructive item is drawn in the failed hue. */
  danger?: boolean
} & ({ onSelect: () => void; href?: never } | { href: string; onSelect?: never })

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
          {items.map((item, index) => (
            <DropdownMenu.Item
              key={index}
              asChild={item.href !== undefined}
              disabled={item.disabled}
              onSelect={item.onSelect}
              aria-current={item.current ? 'true' : undefined}
              className={item.danger ? 'ui-menu-item ui-menu-item-danger' : 'ui-menu-item'}
            >
              {item.href !== undefined ? (
                <a href={item.href}>
                  <span className="ui-menu-copy">
                    <span>{item.label}</span>{item.detail && ' '}
                    {item.detail && <span className="ui-menu-detail">{item.detail}</span>}
                  </span>
                  {item.current && <Check className="ui-menu-check" aria-hidden />}
                </a>
              ) : item.label}
            </DropdownMenu.Item>
          ))}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  )
}
