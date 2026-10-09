// The page list of the Memory page: the scope, the view and the
// change-time groups. The daemon narrows the list by the view and the
// search. The module knows nothing about
// React, so the rules are tested on their own.

import type { components } from '../../api/schema'

export type MemoryPageDto = components['schemas']['MemoryPageDto']

/** The views of a scope. Changes lists the commits of the scope. */
export type MemoryView = 'pages' | 'changes' | 'procedures'

export type MemoryScope = { kind: 'agent'; agentId: string } | { kind: 'shared' }

/** The API scope word: `agent:<id>` or `shared`. */
export function scopeParam(scope: MemoryScope): string {
  return scope.kind === 'shared' ? 'shared' : `agent:${scope.agentId}`
}

export function parseScope(value: string): MemoryScope | null {
  if (value === 'shared') return { kind: 'shared' }
  const agentId = value.startsWith('agent:') ? value.slice('agent:'.length) : ''
  return agentId === '' ? null : { kind: 'agent', agentId }
}

/** The kind the daemon narrows a view to; none for every page. */
export function viewKind(view: MemoryView): string | undefined {
  return view === 'procedures' ? 'Procedure' : undefined
}

export interface PageGroup {
  label: string
  pages: MemoryPageDto[]
}

const DAY = 86_400_000

function startOfDay(at: number): number {
  const day = new Date(at)
  day.setHours(0, 0, 0, 0)
  return day.getTime()
}

/** The pages newest first, in three groups by change time. */
export function groupPages(pages: readonly MemoryPageDto[], now: number = Date.now()): PageGroup[] {
  const today = startOfDay(now)
  const groups: PageGroup[] = [
    { label: 'Changed today', pages: [] },
    { label: 'Changed this week', pages: [] },
    { label: 'Earlier', pages: [] },
  ]
  for (const page of [...pages].sort((left, right) => right.changed_at - left.changed_at)) {
    const index = page.changed_at >= today ? 0 : page.changed_at >= today - 6 * DAY ? 1 : 2
    groups[index]?.pages.push(page)
  }
  return groups.filter((group) => group.pages.length > 0)
}

/** `09:05` today, `Yesterday`, else `Sep 12`. */
export function changeTimeLabel(at: number, now: number = Date.now()): string {
  const today = startOfDay(now)
  if (at >= today) {
    return new Date(at).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' })
  }
  if (at >= today - DAY) return 'Yesterday'
  return new Date(at).toLocaleDateString('en-US', { month: 'short', day: 'numeric' })
}

/** The phone names a day this week before it needs a calendar date. */
export function phoneTimeLabel(at: number, now: number = Date.now()): string {
  const today = startOfDay(now)
  return at < today - DAY && at >= today - 6 * DAY
    ? new Date(at).toLocaleDateString('en-US', { weekday: 'short' })
    : changeTimeLabel(at, now)
}
