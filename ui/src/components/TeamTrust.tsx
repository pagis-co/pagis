import type { components } from '../api/schema'
import { Badge, Frame, Row, SectionLabel } from '../primitives'

const roleNames = { manager: 'Manager', responder: 'Responder', reader: 'Reader' }

export function TeamTrust({ members }: {
  members: components['schemas']['TeamTrustDto'][]
}) {
  return (
    <section aria-label="Team trust">
      <SectionLabel>Team</SectionLabel>
      <p className="settings-hint">
        Managers' words are instructions. Responders' and Readers' words are requests.
        Team trust follows current membership.
      </p>
      <Frame>
        {members.map((member) => (
          <Row key={member.person_id}>
            <span>{member.name ?? 'Person'}</span>
            <span>{roleNames[member.role]}</span>
            <Badge>Team</Badge>
          </Row>
        ))}
      </Frame>
    </section>
  )
}
