// The page as the daemon holds it: the Memory page reads the
// compiled truth, the Facts table, the open Schedules and the Timeline
// from the rendered Subject Page.

import { describe, expect, it } from 'vitest'

import { pageProse, parseSourceReference, parseSubjectPage } from './subjectPage'

const PAGE = [
  '---',
  'title: Priya Sharma',
  'kind: Person',
  '---',
  '# Person: Priya Sharma',
  '',
  'Head of Procurement at Northwind.',
  '',
  '## Facts',
  '',
  '| Claim | Kind | Source reference |',
  '| --- | --- | --- |',
  '| priya@northwind.co | Email | gmail:con_1:m1:1 |',
  '| Buyer \\| lead | Role | gmail:con_1:m1:1 |',
  '| ~~Analyst~~<br>superseded by #2 | Role | gmail:con_1:m0:1 |',
  '',
  '## Schedules',
  '',
  '| When | What for | Schedule id |',
  '| --- | --- | --- |',
  '| 1758272400000 | Send the revised quote | sch_1 |',
  '',
  '---',
  '',
  '## Timeline',
  '',
  '### Entry',
  '',
  'Source reference: `gmail:con_1:m1:1`',
  '',
  'Source time: 1757405700000',
  '',
  '> First message on the renewal.',
  '>',
  '> cc Mark.',
  '',
].join('\n')

describe('parseSubjectPage', () => {
  it('reads the truth without the front matter and the heading', () => {
    expect(parseSubjectPage(PAGE)?.truth).toBe('Head of Procurement at Northwind.')
  })

  it('reads the Facts rows with their status', () => {
    const facts = parseSubjectPage(PAGE)?.facts ?? []
    expect(facts.map((fact) => [fact.kind, fact.claim, fact.status])).toEqual([
      ['Email', 'priya@northwind.co', 'active'],
      ['Role', 'Buyer | lead', 'active'],
      ['Role', 'Analyst', 'superseded'],
    ])
    expect(facts[2]?.note).toBe('superseded by #2')
  })

  it('reads the open Schedules', () => {
    expect(parseSubjectPage(PAGE)?.schedules).toEqual([
      { dueAt: 1758272400000, purpose: 'Send the revised quote', id: 'sch_1' },
    ])
  })

  it('reads the Timeline entries with their words as they arrived', () => {
    expect(parseSubjectPage(PAGE)?.timeline).toEqual([
      {
        sourceReference: 'gmail:con_1:m1:1',
        sourceTime: 1757405700000,
        words: 'First message on the renewal.\n\ncc Mark.',
      },
    ])
  })

  it('reads a page with no truth, no rows and no entries', () => {
    const empty = parseSubjectPage(
      '## Facts\n\n| Claim | Kind | Source reference |\n| --- | --- | --- |\n\n## Schedules\n\n| When | What for | Schedule id |\n| --- | --- | --- |\n\n---\n\n## Timeline\n',
    )
    expect(empty).toEqual({ truth: '', facts: [], schedules: [], timeline: [] })
  })

  it('gives null for a page without the Subject Page layout', () => {
    expect(parseSubjectPage('# Household\n\nTwo kids.\n')).toBeNull()
  })
})

describe('pageProse', () => {
  it('drops the front matter and the first heading', () => {
    expect(pageProse('---\ntitle: Household\n---\n# Household\n\nTwo kids.\n')).toBe(
      'Two kids.',
    )
  })

  it('keeps a page with neither', () => {
    expect(pageProse('Two kids.\n')).toBe('Two kids.')
  })
})

describe('parseSourceReference', () => {
  it('splits resource, connection and item', () => {
    expect(parseSourceReference('gmail:con_1:m1:3')).toEqual({
      resource: 'gmail',
      connectionId: 'con_1',
      itemId: 'm1',
    })
  })

  it('gives nulls for the parts a reference does not name', () => {
    expect(parseSourceReference('thread')).toEqual({
      resource: 'thread',
      connectionId: null,
      itemId: null,
    })
  })
})
