import * as RadixSelect from '@radix-ui/react-select'
import { Check, ChevronDown } from 'lucide-react'

import { cx } from './cx'
import './select.css'

export interface SelectItem {
  value: string
  label: string
  disabled?: boolean
}

export interface SelectProps {
  value: string
  onValueChange: (value: string) => void
  items: SelectItem[]
  /** Names the control. A Radix trigger is a button, which a wrapping
   * `<label>` does not name, so every use passes this. */
  label: string
  placeholder?: string
  disabled?: boolean
  id?: string
  className?: string
}

/** A list of one choice, on the Radix Select: keyboard, typeahead and
 * dismissal come from the primitive. */
export function Select({
  value,
  onValueChange,
  items,
  label,
  placeholder = 'Choose…',
  disabled,
  id,
  className,
}: SelectProps) {
  return (
    <RadixSelect.Root value={value} onValueChange={onValueChange} disabled={disabled}>
      <RadixSelect.Trigger
        id={id}
        aria-label={label}
        className={cx('ui-select-trigger', className)}
      >
        <RadixSelect.Value placeholder={placeholder} />
        <RadixSelect.Icon>
          <ChevronDown size={14} aria-hidden focusable="false" />
        </RadixSelect.Icon>
      </RadixSelect.Trigger>
      <RadixSelect.Portal>
        <RadixSelect.Content className="ui-select-content" position="popper" sideOffset={4}>
          <RadixSelect.Viewport>
            {items.map((item) => (
              <RadixSelect.Item
                key={item.value}
                value={item.value}
                disabled={item.disabled}
                className="ui-select-item"
              >
                <RadixSelect.ItemText>{item.label}</RadixSelect.ItemText>
                <RadixSelect.ItemIndicator className="ui-select-indicator">
                  <Check size={14} aria-hidden focusable="false" />
                </RadixSelect.ItemIndicator>
              </RadixSelect.Item>
            ))}
          </RadixSelect.Viewport>
        </RadixSelect.Content>
      </RadixSelect.Portal>
    </RadixSelect.Root>
  )
}
