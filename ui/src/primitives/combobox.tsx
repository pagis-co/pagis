import type { InputHTMLAttributes } from 'react'
import * as Popover from '@radix-ui/react-popover'
import { useCombobox } from 'downshift'
import { RemoveScroll } from 'react-remove-scroll'

import { cx } from './cx'
import { Input } from './input'
import './combobox.css'

export type ComboboxProps = Omit<
  InputHTMLAttributes<HTMLInputElement>,
  'value' | 'onChange' | 'list'
> & {
  value: string
  onValueChange: (value: string) => void
  /** The choices. The list shows the ones that contain the typed text. */
  items: string[]
  /** Names the field. */
  label: string
}

/** A field that takes typed text and offers the items that match it.
 * Downshift gives the combobox keyboard and ARIA behaviour. The list is
 * a Radix Popover in a portal: it stays in the window and scrolls when
 * the items do not fit, which a native `<datalist>` does not do. */
export function Combobox({
  value,
  onValueChange,
  items,
  label,
  className,
  onKeyDown,
  ...rest
}: ComboboxProps) {
  const query = value.trim().toLowerCase()
  const matches = items.filter((item) => item.toLowerCase().includes(query))
  const {
    isOpen,
    highlightedIndex,
    closeMenu,
    getInputProps,
    getMenuProps,
    getItemProps,
  } = useCombobox({
    items: matches,
    inputValue: value,
    selectedItem: null,
    onInputValueChange: ({ inputValue }) => onValueChange(inputValue),
    onSelectedItemChange: ({ selectedItem }) => {
      if (selectedItem !== null) onValueChange(selectedItem)
    },
  })
  const open = isOpen && matches.length > 0

  return (
    <Popover.Root open={open} onOpenChange={(next) => !next && closeMenu()}>
      <Popover.Anchor asChild>
        <Input
          {...getInputProps({
            ...rest,
            'aria-label': label,
            className: cx('ui-combobox-input', className),
            onKeyDown: (event) => {
              // Enter on a highlighted item chooses it; the field's own
              // Enter (for example, to submit) does not run.
              if (event.key === 'Enter' && open && highlightedIndex >= 0) return
              onKeyDown?.(event)
            },
          })}
        />
      </Popover.Anchor>
      <Popover.Portal>
        <Popover.Content
          className="ui-combobox-content"
          align="start"
          sideOffset={4}
          onOpenAutoFocus={(event) => event.preventDefault()}
          onCloseAutoFocus={(event) => event.preventDefault()}
        >
          {/* A dialog blocks the wheel outside itself, and the portal puts
           * the list outside the dialog. The newest lock wins, so this one
           * lets the list scroll. Radix Select does the same. */}
          <RemoveScroll allowPinchZoom>
            <ul
              className="ui-combobox-list"
              {...getMenuProps({ 'aria-label': label }, { suppressRefError: true })}
            >
            {matches.map((item, index) => (
              <li
                key={item}
                className="ui-combobox-item"
                data-highlighted={index === highlightedIndex ? '' : undefined}
                {...getItemProps({ item, index })}
              >
                {item}
              </li>
            ))}
            </ul>
          </RemoveScroll>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  )
}
