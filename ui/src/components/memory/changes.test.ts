import { describe, expect, it } from 'vitest'

import {
  authorName,
  changePaths,
  groupChanges,
  revertedShas,
  sentence,
  sourceLabel,
  type MemoryFeedItem,
} from './changes'

const NOW = new Date(2026, 8, 16, 12, 0).getTime()
const HOUR = 3_600_000

function item(fields: Partial<MemoryFeedItem>): MemoryFeedItem {
  return {
    id: 'e1',
    kind: 'committed',
    sha: 'sha-1',
    agent_id: 'ag1',
    agent_name: 'Sage',
    scopes: ['private'],
    files: [],
    titles: [],
    message: 'Noted',
    source_scoped: false,
    created_at: NOW,
    source_kind: 'thread',
    ...fields,
  }
}

describe('sourceLabel', () => {
  it('names where a change came from', () => {
    expect(sourceLabel(item({ source_kind: 'thread' }))).toBe('from a thread')
    expect(sourceLabel(item({ source_kind: 'sync' }))).toBe('from a sync arrival')
    expect(sourceLabel(item({ source_kind: 'backfill' }))).toBe('Backfill')
    expect(sourceLabel(item({ source_kind: 'revert' }))).toBe('a revert')
  })

  it('names nothing for a change with no Run', () => {
    expect(sourceLabel(item({ source_kind: null }))).toBeNull()
  })
})

describe('authorName', () => {
  it('names the Agent of a commit and You for a revert', () => {
    expect(authorName(item({}))).toBe('Sage')
    expect(authorName(item({ kind: 'reverted', agent_name: null }))).toBe('You')
    expect(authorName(item({ agent_id: null, agent_name: null }))).toBe('You')
  })
})

describe('sentence', () => {
  it('continues the Agent name with the commit message', () => {
    expect(sentence(item({ message: 'Added the Friday deadline' }))).toBe('added the Friday deadline')
    expect(sentence(item({ message: 'API keys rotated' }))).toBe('API keys rotated')
  })

  it('says what a revert did', () => {
    expect(sentence(item({ kind: 'reverted', message: '' }))).toBe('reverted a memory change')
  })
})

describe('changePaths', () => {
  const files = ['private/subjects/t1.md', 'shared/household.md', 'private/MEMORY.md']

  it('keeps the files of a private scope as scope-relative paths', () => {
    expect(changePaths(item({ files }), 'agent:ag1')).toEqual(['subjects/t1.md', 'MEMORY.md'])
  })

  it('keeps the shared files for Shared', () => {
    expect(changePaths(item({ files }), 'shared')).toEqual(['household.md'])
  })
})

describe('groupChanges', () => {
  it('groups the changes by day, newest first, and drops Schedule entries', () => {
    const groups = groupChanges(
      [
        item({ id: 'old', created_at: new Date(2026, 8, 12, 9, 0).getTime() }),
        item({ id: 'today', created_at: NOW - HOUR }),
        item({ id: 'schedule', kind: 'schedule', sha: '' }),
        item({ id: 'yesterday', created_at: NOW - 20 * HOUR }),
        item({ id: 'later today', created_at: NOW }),
      ],
      NOW,
    )

    expect(groups.map((group) => [group.label, group.items.map((entry) => entry.id)])).toEqual([
      ['Today', ['later today', 'today']],
      ['Yesterday', ['yesterday']],
      ['Sep 12', ['old']],
    ])
  })
})

describe('revertedShas', () => {
  it('collects the commits that a revert undoes', () => {
    const shas = revertedShas([
      item({ kind: 'reverted', reverted_sha: 'sha-0' }),
      item({ kind: 'committed', sha: 'sha-1' }),
    ])

    expect([...shas]).toEqual(['sha-0'])
  })
})
