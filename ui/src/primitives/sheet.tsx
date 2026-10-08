import * as Dialog from '@radix-ui/react-dialog'
import { useRef, type ReactNode } from 'react'
import { Button } from './button'
import { IconButton } from './icon-button'
import { X } from 'lucide-react'
import './sheet.css'

export interface SheetProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  cancelLabel?: string
  showHeader?: boolean
  action?: { label: string; onSelect: () => void; disabled?: boolean }
  children: ReactNode
  footer?: ReactNode
}

export function Sheet({
  open,
  onOpenChange,
  title,
  cancelLabel = 'Cancel',
  showHeader = true,
  action,
  children,
  footer,
}: SheetProps) {
  const start = useRef<number | null>(null)
  const opener = useRef<HTMLElement | null>(null)
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="ui-sheet-scrim" />
        <Dialog.Content
          className={`ui-sheet${showHeader ? '' : ' ui-sheet-headerless'}`}
          aria-describedby={undefined}
          onOpenAutoFocus={() => {
            opener.current = document.activeElement as HTMLElement
          }}
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            opener.current?.focus()
          }}
        >
          <div
            className="ui-sheet-grabber"
            aria-hidden
            onPointerDown={(event) => {
              start.current = event.clientY
              event.currentTarget.setPointerCapture(event.pointerId)
            }}
            onPointerUp={(event) => {
              if (start.current !== null && event.clientY - start.current >= 120)
                onOpenChange(false)
              start.current = null
            }}
          />
          {showHeader ? (
            <header className="ui-sheet-header">
              <Dialog.Close asChild>
                <Button variant="link">{cancelLabel}</Button>
              </Dialog.Close>
              <Dialog.Title className="ui-sheet-title">{title}</Dialog.Title>
              {action ? (
                <Button variant="link" disabled={action.disabled} onClick={action.onSelect}>
                  {action.label}
                </Button>
              ) : (
                <span />
              )}
            </header>
          ) : (
            <>
              <Dialog.Title className="ui-sheet-title-hidden">{title}</Dialog.Title>
              <Dialog.Close asChild>
                <IconButton icon={X} label="Close" variant="ghost" className="ui-sheet-close" />
              </Dialog.Close>
            </>
          )}
          <div className="ui-sheet-body">{children}</div>
          {footer && <footer className="ui-sheet-footer">{footer}</footer>}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}
