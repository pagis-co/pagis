import { ChevronLeft, type LucideIcon } from 'lucide-react'
import { createContext, useContext, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { Button } from '../../primitives'
import './phone.css'

export const HeaderActions = createContext<HTMLElement | null>(null)

export function PhoneHeaderAction({ children }: { children: ReactNode }) {
  const target = useContext(HeaderActions)
  return target ? createPortal(children, target) : null
}

export function LargeTitle({
  title,
  hint,
  actions,
}: {
  title: string
  hint?: ReactNode
  actions?: ReactNode
}) {
  return (
    <header className="large-title">
      <div>
        <h1>{title}</h1>
        {hint && <p>{hint}</p>}
      </div>
      {actions}
    </header>
  )
}

export function NavBar({
  back,
  backIcon: BackIcon = ChevronLeft,
  actions,
  children,
}: {
  back: { label: string; onBack: () => void }
  backIcon?: LucideIcon
  actions?: ReactNode
  children?: ReactNode
}) {
  return (
    <header className="nav-bar">
      <Button variant="link" onClick={back.onBack} aria-label={back.label}>
        <BackIcon size={20} aria-hidden />
        {children ? null : back.label}
      </Button>
      {children}
      <div className="nav-bar-actions">{actions}</div>
    </header>
  )
}
