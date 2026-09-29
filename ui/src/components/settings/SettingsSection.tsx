import type { ReactNode } from 'react'

import './SettingsSection.css'

export interface SettingsSectionProps {
  title: string
  /** One sentence that says what the section holds. */
  lead: string
  /** The one primary action of the section, at the right of the title. */
  action?: ReactNode
  /** The framed lists. */
  children: ReactNode
  /** The closing line that says what the page cannot do. */
  hint?: ReactNode
}

/** The settings grammar: a title line with a lead and
 * at most one primary action, the framed lists, and a closing hint. */
export function SettingsSection({ title, lead, action, children, hint }: SettingsSectionProps) {
  return (
    <section className="settings-section">
      <div className="settings-section-head">
        <h3>{title}</h3>
        <span className="settings-section-lead">{lead}</span>
        {action}
      </div>
      {children}
      {hint === undefined ? null : <p className="settings-section-hint">{hint}</p>}
    </section>
  )
}
