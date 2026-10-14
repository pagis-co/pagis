import type { components } from '../api/schema'
import { Badge } from '../primitives'
import './PersonNotice.css'

export function PersonNotice({ notice }: {
  notice: components['schemas']['PersonNoticeDto'] | null | undefined
}) {
  if (!notice) return null
  return (
    <aside className="person-notice" role="note">
      <Badge tone={notice.tone ?? 'neutral'}>{notice.label}</Badge>
      <p>{notice.text}</p>
      {notice.link && <a href={notice.link.url}>{notice.link.label}</a>}
    </aside>
  )
}
