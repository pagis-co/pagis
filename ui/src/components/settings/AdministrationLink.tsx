// The one place the product names the installation's administration.
//
// The people, the spend, the providers, the System Settings and the
// Plugins of the installation answer on the administration port alone
// (ADR-0024), so the product draws none of them. An administrator gets
// this link instead. Where the port binds loopback, only the machine
// that runs Pagis reaches it, so the section says how to reach it from
// another machine: an SSH tunnel to the same port.

import type { ApiClient } from '../../api/client'
import { Frame, Row } from '../../primitives'
import { useUser } from '../../queries'
import { SettingsSection } from './SettingsSection'

/** The port of an origin, with the scheme's default where it names
 *  none. */
function portOf(origin: string): string {
  const url = new URL(origin)
  return url.port !== '' ? url.port : url.protocol === 'https:' ? '443' : '80'
}

export function AdministrationLink({ api }: { api: ApiClient }) {
  const address = useUser(api).data?.administration
  if (address === undefined || address === null) return null
  const port = portOf(address.origin)

  return (
    <SettingsSection
      title="Administration"
      lead="The people, the spend, the providers, the system settings and the plugins of this installation."
      hint={
        address.loopback ? (
          <>
            The Administration Interface answers only on the machine that runs Pagis. From
            another machine, open an SSH tunnel to port {port} first:{' '}
            <code>
              ssh -L {port}:127.0.0.1:{port} you@your-server
            </code>
            , then open the link.
          </>
        ) : undefined
      }
    >
      <Frame>
        <Row>
          <a href={`${address.origin}/`} target="_blank" rel="noreferrer">
            Open the Administration Interface
          </a>
          <span className="settings-section-lead">{address.origin}</span>
        </Row>
      </Frame>
    </SettingsSection>
  )
}
