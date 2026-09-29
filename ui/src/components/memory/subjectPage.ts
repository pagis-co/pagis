// The Subject Page as the daemon renders it (ADR-0007): compiled
// truth, the Facts table and the open Schedules above the rule, and the
// Timeline below it. The Memory page reads the four parts from
// the text of `GET /api/v1/memory/file`. The layout is the one
// `pagis_core::subject_page::SubjectPage::render` writes.

const FACTS_HEADING = '\n## Facts\n'
const SCHEDULES_HEADING = '\n## Schedules\n'
const TIMELINE_BOUNDARY = '\n---\n\n## Timeline\n'
const ENTRY_HEADING = '\n### Entry\n'

export type FactStatus = 'active' | 'superseded' | 'forgotten'

export interface Fact {
  claim: string
  kind: string
  status: FactStatus
  /** Why a row is not active: `superseded by #2`, `forgotten: ...`. */
  note: string | null
  sourceReference: string
}

export interface OpenSchedule {
  /** The next due time, in epoch milliseconds. */
  dueAt: number
  purpose: string
  id: string
}

export interface TimelineEntry {
  sourceReference: string
  sourceTime: number
  /** The source words as they arrived. */
  words: string
}

export interface SubjectPage {
  truth: string
  facts: Fact[]
  schedules: OpenSchedule[]
  timeline: TimelineEntry[]
}

/** The parts of a `resource:connection_id:item:version` reference. */
export interface SourceReference {
  resource: string
  connectionId: string | null
  itemId: string | null
}

export function parseSourceReference(reference: string): SourceReference {
  const [resource = '', connectionId, itemId] = reference.split(':')
  return {
    resource,
    connectionId: connectionId === undefined || connectionId === '' ? null : connectionId,
    itemId: itemId === undefined || itemId === '' ? null : itemId,
  }
}

/** The body of a page without its front matter and its first `#`
 *  heading, which the page header already shows. */
export function pageProse(content: string): string {
  let body = content.trimStart()
  if (body.startsWith('---\n')) {
    const end = body.indexOf('\n---\n', 4)
    if (end !== -1) body = body.slice(end + 5)
  }
  const lines = body.split('\n')
  const heading = lines.findIndex((line) => line.trim() !== '')
  if (heading !== -1 && lines[heading]?.startsWith('# ')) lines.splice(heading, 1)
  return lines.join('\n').trim()
}

/** The four parts of a Subject Page, or `null` for a page with another
 *  layout (a shared note, a procedure). */
export function parseSubjectPage(content: string): SubjectPage | null {
  const text = `\n${content}`
  const facts = text.indexOf(FACTS_HEADING)
  const schedules = text.indexOf(SCHEDULES_HEADING, facts)
  const boundary = text.indexOf(TIMELINE_BOUNDARY, schedules)
  if (facts === -1 || schedules === -1 || boundary === -1) return null
  return {
    truth: pageProse(text.slice(0, facts)),
    facts: tableRows(text.slice(facts, schedules)).map(factRow),
    schedules: tableRows(text.slice(schedules, boundary)).map(([when, purpose, id]) => ({
      dueAt: Number(when),
      purpose: purpose ?? '',
      id: id ?? '',
    })),
    timeline: text
      .slice(boundary + TIMELINE_BOUNDARY.length)
      .split(ENTRY_HEADING)
      .slice(1)
      .map(timelineEntry),
  }
}

function factRow(cells: string[]): Fact {
  const [claimCell = '', kind = '', sourceReference = ''] = cells
  const struck = /^~~([\s\S]*)~~\n([\s\S]*)$/.exec(claimCell)
  if (struck === null) {
    return { claim: claimCell, kind, status: 'active', note: null, sourceReference }
  }
  const note = struck[2] ?? ''
  return {
    claim: struck[1] ?? '',
    kind,
    status: note.startsWith('forgotten') ? 'forgotten' : 'superseded',
    note,
    sourceReference,
  }
}

/** The body rows of the Markdown table in a section: the header row and
 *  the separator row are dropped, and each cell is unescaped. */
function tableRows(section: string): string[][] {
  return section
    .split('\n')
    .filter((line) => line.startsWith('|'))
    .slice(2)
    .map(splitRow)
}

function splitRow(line: string): string[] {
  const cells: string[] = []
  let cell = ''
  // Skip the leading pipe; each later unescaped pipe ends a cell.
  for (let index = 1; index < line.length; index += 1) {
    const char = line[index]
    if (char === '\\' && index + 1 < line.length) {
      cell += line[index + 1]
      index += 1
    } else if (char === '|') {
      cells.push(cell.trim().replaceAll('<br>', '\n'))
      cell = ''
    } else {
      cell += char
    }
  }
  return cells
}

function timelineEntry(block: string): TimelineEntry {
  const reference = /^Source reference: `((?:\\`|[^`])*)`$/m.exec(block)?.[1] ?? ''
  const time = /^Source time: (-?\d+)$/m.exec(block)?.[1]
  const words = block
    .split('\n')
    .filter((line) => line.startsWith('>'))
    .map((line) => line.replace(/^> ?/, ''))
    .join('\n')
  return {
    sourceReference: reference.replaceAll('\\`', '`'),
    sourceTime: Number(time ?? 0),
    words,
  }
}
