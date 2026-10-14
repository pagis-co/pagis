import type { components } from '../api/schema'
import { Badge, cx } from '../primitives'
import './PersonNotice.css'

export function PersonNotice({ notice, className }: {
  notice: components['schemas']['PersonNoticeDto'] | null | undefined
  className?: string
}) {
  if (!notice) return null
  return (
    <aside className={cx('person-notice', className)} role="note">
      <Badge tone={notice.tone ?? 'neutral'}>{notice.label}</Badge>
      <p>{notice.text}</p>
      {notice.link && <a href={notice.link.url}>{notice.link.label}</a>}
    </aside>
  )
}
