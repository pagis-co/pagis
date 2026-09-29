import { createContext, useCallback, useContext, useMemo, useState } from 'react'
import type { ReactNode } from 'react'
import * as RadixToast from '@radix-ui/react-toast'
import { X } from 'lucide-react'

import { IconButton } from './icon-button'
import './toast.css'

export type ToastTone = 'neutral' | 'working' | 'failed'

export interface ToastMessage {
  title: string
  description?: string
  tone?: ToastTone
}

interface ToastState extends ToastMessage {
  id: number
}

const ToastContext = createContext<((message: ToastMessage) => void) | null>(null)

/** Raises a toast. It throws outside a `ToastProvider`, so a missing
 * provider fails at the first call and not silently. */
export function useToast(): (message: ToastMessage) => void {
  const notify = useContext(ToastContext)
  if (notify === null) throw new Error('useToast needs a ToastProvider above it')
  return notify
}

/** Holds the live toasts and their viewport. */
export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastState[]>([])
  const notify = useCallback((message: ToastMessage) => {
    setToasts((current) => [...current, { ...message, id: Date.now() + current.length }])
  }, [])
  const dismiss = useCallback((id: number) => {
    setToasts((current) => current.filter((toast) => toast.id !== id))
  }, [])
  const value = useMemo(() => notify, [notify])

  return (
    <ToastContext.Provider value={value}>
      <RadixToast.Provider swipeDirection="right">
        {children}
        {toasts.map((toast) => (
          <RadixToast.Root
            key={toast.id}
            className={`ui-toast ui-toast-${toast.tone ?? 'neutral'}`}
            open
            onOpenChange={(open) => {
              if (!open) dismiss(toast.id)
            }}
          >
            <RadixToast.Title className="ui-toast-title">{toast.title}</RadixToast.Title>
            {toast.description === undefined ? null : (
              <RadixToast.Description className="ui-toast-description">
                {toast.description}
              </RadixToast.Description>
            )}
            <RadixToast.Close asChild>
              <IconButton icon={X} label="Dismiss" size="sm" variant="ghost" />
            </RadixToast.Close>
          </RadixToast.Root>
        ))}
        <RadixToast.Viewport className="ui-toast-viewport" />
      </RadixToast.Provider>
    </ToastContext.Provider>
  )
}
