import { ChevronsUpDown } from 'lucide-react'
import type { components } from '../api/schema'
import { Button, InitialTile, Menu } from '../primitives'
import './org-head.css'

export function OrgHead({ org, role }: {
  org: components['schemas']['OrgDto']
  role: string
}) {
  const content = (
    <>
      <InitialTile name={org.name} />
      <span className="org-copy">
        <span className="org-name">{org.name}</span>{' '}
        <span className="org-role">
          {role === 'administrator' ? 'Administrator' : 'Member'}
        </span>
      </span>
    </>
  )
  if (org.entries.length === 0) {
    return <div className="org-head org-head-static">{content}</div>
  }
  return (
    <Menu
      label="Org menu"
      trigger={
        <Button variant="ghost" className="org-head">
          {content}
          <ChevronsUpDown className="org-menu-icon" aria-hidden />
        </Button>
      }
      items={org.entries.map((entry) => ({
        label: entry.label,
        href: entry.url,
        detail: entry.detail ?? undefined,
        current: entry.current,
      }))}
    />
  )
}
