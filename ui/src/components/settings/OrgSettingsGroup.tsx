import type { components } from '../../api/schema'
import { ADMINISTRATION_VIEWS } from '../../administration/views'
import { ButtonLink, Frame, Row, SectionLabel } from '../../primitives'
import './OrgSettingsGroup.css'

export type AdministrationAddress = components['schemas']['AdministrationAddress']

/** Org links open the Administration Interface or a configured URL. */
export function OrgSettingsGroup({
  administration,
  isAdministrator,
  phone = false,
}: {
  administration?: AdministrationAddress | null
  isAdministrator: boolean
  phone?: boolean
}) {
  if (!isAdministrator || !administration) return null
  const items = [
    ...administration.sections.flatMap((section) => {
      const view = ADMINISTRATION_VIEWS.find((view) => view.value === section)
      return view ? [{
        label: view.label,
        url: `${administration.origin}${administration.path_prefix}/${section}`,
      }] : []
    }),
    ...administration.links,
  ]
  const url = new URL(administration.origin)
  const port = url.port || (url.protocol === 'https:' ? '443' : '80')
  const links = items.map((item, index) => phone ? (
    <Row key={index} externalHref={item.url} chevron>{item.label}</Row>
  ) : (
    <ButtonLink
      key={index}
      href={item.url}
      variant="ghost"
      shape="nav"
      className="settings-nav-item"
    >
      {item.label}
    </ButtonLink>
  ))
  return (
    <div
      className={phone ? 'phone-section' : 'settings-nav-group'}
      role="group"
      aria-label="Org"
    >
      <SectionLabel>Org</SectionLabel>
      {phone ? <Frame>{links}</Frame> : links}
      {administration.loopback && (
        <p className="org-settings-tunnel">
          The Administration Interface answers only on the machine that runs Pagis. From
          another machine, open an SSH tunnel to port {port} first:{' '}
          <code>ssh -L {port}:127.0.0.1:{port} you@your-server</code>
          , then open the link.
        </p>
      )}
    </div>
  )
}
