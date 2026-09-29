import type { ReactNode } from 'react'
import * as RadixDialog from '@radix-ui/react-dialog'
import { X } from 'lucide-react'

import { Button } from './button'
import './dialog.css'

export interface DialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  description?: string
  children?: ReactNode
  /** The row of actions under the body. */
  footer?: ReactNode
  /** The control that opens the modal. Radix gives the focus back to it
   * on close, so pass it here instead of drawing it beside the modal. */
  trigger?: ReactNode
}

/** A modal on the Radix Dialog: the focus is trapped, Escape and the
 * scrim close it and the focus returns to the opener. */
export function Dialog({
  open,
  onOpenChange,
  title,
  description,
  children,
  footer,
  trigger,
}: DialogProps) {
  return (
    <RadixDialog.Root open={open} onOpenChange={onOpenChange}>
      {trigger === undefined ? null : (
        <RadixDialog.Trigger asChild>{trigger}</RadixDialog.Trigger>
      )}
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="ui-dialog-scrim" />
        <RadixDialog.Content className="ui-dialog">
          <div className="ui-dialog-header">
            <RadixDialog.Title className="ui-dialog-title">{title}</RadixDialog.Title>
            <RadixDialog.Close asChild>
              <Button variant="ghost" aria-label="Close" className="ui-icon-button">
                <X size={16} aria-hidden focusable="false" />
              </Button>
            </RadixDialog.Close>
          </div>
          {description === undefined ? null : (
            <RadixDialog.Description className="ui-dialog-description">
              {description}
            </RadixDialog.Description>
          )}
          <div className="ui-dialog-body">{children}</div>
          {footer === undefined ? null : <div className="ui-dialog-footer">{footer}</div>}
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  )
}
