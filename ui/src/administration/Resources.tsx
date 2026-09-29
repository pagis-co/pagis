// What each person's sprites take of the host: containers,
// volumes, disk and awake Computers.
//
// Every Docker object carries the owner label of its tenant, so these
// figures are one person's and never the machine's total attributed to
// somebody. Docker that cannot answer leaves a figure out rather than
// failing the page.

import type { ApiClient, PersonResourcesDto } from '../api/client'
import { Badge, Frame, Row, SectionLabel } from '../primitives'
import { useInstallationResources } from '../queries'
import { personLabel } from '../components/settings/People'

/** Bytes as a person reads them. */
export function bytes(count: number): string {
  if (count < 1024) return `${count} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let value = count / 1024
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`
}

function PersonRow({ item }: { item: PersonResourcesDto }) {
  return (
    <Row className="resources-row">
      <span className="resources-person">{personLabel(item.person)}</span>
      <span className="resources-count">
        {item.containers} {item.containers === 1 ? 'container' : 'containers'}
      </span>
      <span className="resources-count">
        {item.volumes} {item.volumes === 1 ? 'volume' : 'volumes'}
      </span>
      <span className="resources-disk">
        {item.volume_bytes === null || item.volume_bytes === undefined
          ? 'disk unread'
          : bytes(item.volume_bytes)}
      </span>
      {item.awake_computers > 0 && (
        <Badge tone="working">
          {item.awake_computers} awake
        </Badge>
      )}
    </Row>
  )
}

export function Resources({ api }: { api: ApiClient }) {
  const resources = useInstallationResources(api)

  // No figure shows before the read answers: an empty list and zero
  // caps would say something that is not so.
  if (!resources.data) {
    return (
      <section className="administration-section">
        <div className="administration-title">
          <h2>Resources</h2>
        </div>
        <p className="administration-note">
          {resources.isError ? 'The resources could not be read.' : 'Reading…'}
        </p>
      </section>
    )
  }
  const data = resources.data

  return (
    <section className="administration-section">
      <div className="administration-title">
        <h2>Resources</h2>
        <span>What each person's sprite computers take of this machine.</span>
      </div>
      <Frame hint="Every container and every volume carries the owner label of its tenant, so one person's figures are theirs alone. A container stops on its own after it sits idle.">
        {data.items.length === 0 ? (
          <Row>
            <span className="administration-note">Nobody has a sprite computer yet.</span>
          </Row>
        ) : (
          data.items.map((item) => <PersonRow key={item.person.id} item={item} />)
        )}
      </Frame>

      <SectionLabel>Awake computers</SectionLabel>
      <Frame hint="The cap on awake Computers is an installation setting: it bounds how many run at once, per person and for the whole server.">
        <Row>
          <span className="resources-person">This server</span>
          <span className="resources-count">
            {data.awake_on_server} awake of {data.awake_cap_per_server}
          </span>
          <span className="administration-note">
            {data.awake_cap_per_tenant} at once per person
          </span>
        </Row>
      </Frame>
    </section>
  )
}
