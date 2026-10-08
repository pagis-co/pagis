import * as AlertDialog from '@radix-ui/react-alert-dialog'
import { useRef, type ReactNode } from 'react'
import './sheet.css'

export interface ActionSheetProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  description: ReactNode
  children?: ReactNode
  action?: { label: string; onSelect: () => void; danger?: boolean; disabled?: boolean }
  actions?: { label: string; onSelect: () => void; danger?: boolean; disabled?: boolean }[]
  cancelLabel?: string
}

export function ActionSheet({
  open,
  onOpenChange,
  title,
  description,
  children,
  action,
  actions = [],
  cancelLabel = 'Cancel',
}: ActionSheetProps) {
  const cancel = useRef<HTMLButtonElement>(null)
  const opener = useRef<HTMLElement | null>(null)
  return (
    <AlertDialog.Root open={open} onOpenChange={onOpenChange}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="ui-sheet-scrim" />
        <AlertDialog.Content
          className="ui-action-sheet"
          onOpenAutoFocus={(event) => {
            event.preventDefault()
            opener.current = document.activeElement as HTMLElement
            cancel.current?.focus()
          }}
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            opener.current?.focus()
          }}
        >
          <div className="ui-action-sheet-card">
            <AlertDialog.Title className="ui-action-sheet-title">{title}</AlertDialog.Title>
            <AlertDialog.Description className="ui-action-sheet-description">
              {description}
            </AlertDialog.Description>
            {children && <div className="ui-action-sheet-body">{children}</div>}
            {(action ? [action] : actions).map((item) => (
              <AlertDialog.Action
                key={item.label}
                className={`ui-action-sheet-action${item.danger ? ' ui-action-sheet-danger' : ''}`}
                disabled={item.disabled}
                onClick={item.onSelect}
              >
                {item.label}
              </AlertDialog.Action>
            ))}
          </div>
          <AlertDialog.Cancel ref={cancel} className="ui-action-sheet-cancel">
            {cancelLabel}
          </AlertDialog.Cancel>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  )
}
