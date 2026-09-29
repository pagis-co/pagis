import { AlertCircle, type LucideIcon } from 'lucide-react'
import type { ReactNode } from 'react'

import { Button } from '../primitives'
import './PageState.css'

export function PageState({
  icon: Icon,
  title,
  children,
  onRetry,
}: {
  icon: LucideIcon
  title: string
  children?: ReactNode
  onRetry?: () => void
}) {
  const Symbol = onRetry ? AlertCircle : Icon
  return (
    <div className="page-state" role={onRetry ? 'alert' : 'status'}>
      <span className="page-state-icon"><Symbol size={20} aria-hidden /></span>
      <strong>{title}</strong>
      {children && <div className="page-state-description">{children}</div>}
      {onRetry && <Button onClick={onRetry}>Try again</Button>}
    </div>
  )
}
