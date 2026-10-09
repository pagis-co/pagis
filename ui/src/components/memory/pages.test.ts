// The page list of the Memory page: a view names the kind the
// daemon narrows to, and the groups follow the change time.

import { describe, expect, it } from 'vitest'

import type { MemoryPageDto } from './pages'
import { changeTimeLabel, groupPages, parseScope, viewKind } from './pages'

const NOW = new Date(2026, 8, 16, 12, 0).getTime()
const HOUR = 3_600_000
const DAY = 24 * HOUR

function page(fields: Partial<MemoryPageDto>): MemoryPageDto {
  return {
    scope: 'agent:ag1',
    path: 'subjects/gmail/x.md',
    excerpt: '',
    title: 'Priya Sharma',
    kind: 'Person',
    source_connection_id: 'con_1',
    changed_at: NOW,
    changed_by: 'Sage',
    changed_by_agent_id: 'ag1',
    ...fields,
  }
}

describe('viewKind', () => {
  it('narrows the Procedures view only', () => {
    expect(viewKind('procedures')).toBe('Procedure')
    expect(viewKind('pages')).toBeUndefined()
    expect(viewKind('changes')).toBeUndefined()
  })
})

describe('groupPages', () => {
  it('groups by change time, newest first, and drops an empty group', () => {
    const groups = groupPages(
      [
        page({ path: 'old.md', changed_at: NOW - 30 * DAY }),
        page({ path: 'today.md', changed_at: NOW - HOUR }),
        page({ path: 'week.md', changed_at: NOW - 3 * DAY }),
        page({ path: 'now.md', changed_at: NOW }),
      ],
      NOW,
    )
    expect(groups.map((group) => [group.label, group.pages.map((row) => row.path)])).toEqual([
      ['Changed today', ['now.md', 'today.md']],
      ['Changed this week', ['week.md']],
      ['Earlier', ['old.md']],
    ])
  })

  it('gives no group for no pages', () => {
    expect(groupPages([], NOW)).toEqual([])
  })
})

describe('changeTimeLabel', () => {
  it('says the clock time today', () => {
    expect(changeTimeLabel(new Date(2026, 8, 16, 9, 5).getTime(), NOW)).toBe('09:05')
  })

  it('says Yesterday for yesterday', () => {
    expect(changeTimeLabel(NOW - DAY, NOW)).toBe('Yesterday')
  })

  it('says the month and day before that', () => {
    expect(changeTimeLabel(new Date(2026, 8, 12, 8, 0).getTime(), NOW)).toBe('Sep 12')
  })
})

describe('parseScope', () => {
  it('reads an agent scope and the shared scope', () => {
    expect(parseScope('agent:ag1')).toEqual({ kind: 'agent', agentId: 'ag1' })
    expect(parseScope('shared')).toEqual({ kind: 'shared' })
  })

  it('gives null for any other word', () => {
    expect(parseScope('agent:')).toBeNull()
    expect(parseScope('private')).toBeNull()
  })
})
