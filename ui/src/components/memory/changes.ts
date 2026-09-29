// The rules of the Changes view: where a change came from, who
// made it, the pages it touched in one scope, and the day groups. The
// module knows nothing about React, so the rules are tested on their own.

import type { MemoryFeedItem } from '../../api/client'

export type { MemoryFeedItem }

/** The `kind` filter of the feed. */
export type ChangeKind = 'thread' | 'sync' | 'revert'

export const CHANGE_FILTERS: { kind: ChangeKind | undefined; label: string }[] = [
  { kind: undefined, label: 'All' },
  { kind: 'thread', label: 'From threads' },
  { kind: 'sync', label: 'From sync' },
  { kind: 'revert', label: 'Reverts' },
]

const SOURCE_LABELS: Record<string, string> = {
  thread: 'from a thread',
  sync: 'from a sync arrival',
  backfill: 'Backfill',
  revert: 'a revert',
}

/** Where a change came from; `null` for a change with no Run. */
export function sourceLabel(item: MemoryFeedItem): string | null {
  return item.source_kind == null ? null : (SOURCE_LABELS[item.source_kind] ?? null)
}

/** The Agent that wrote a commit. The user makes every revert. */
export function authorName(item: MemoryFeedItem): string {
  return item.kind === 'reverted' ? 'You' : (item.agent_name ?? 'You')
}

/** The words after the author: the commit message as a continuation
 *  of the sentence. A leading word in capitals (an acronym) stays. */
export function sentence(item: MemoryFeedItem): string {
  if (item.kind === 'reverted') return 'reverted a memory change'
  const [first = '', second = ''] = item.message
  return second === second.toUpperCase() && second !== second.toLowerCase()
    ? item.message
    : first.toLowerCase() + item.message.slice(1)
}

/** The scope-relative paths of the files a change touched in one scope. */
export function changePaths(item: MemoryFeedItem, scope: string): string[] {
  const prefix = scope === 'shared' ? 'shared/' : 'private/'
  return item.files
    .filter((file) => file.startsWith(prefix))
    .map((file) => file.slice(prefix.length))
}

/** The commits that a revert in the list undoes. */
export function revertedShas(items: readonly MemoryFeedItem[]): Set<string> {
  return new Set(
    items.flatMap((item) =>
      item.kind === 'reverted' && item.reverted_sha != null ? [item.reverted_sha] : [],
    ),
  )
}

export interface ChangeGroup {
  label: string
  items: MemoryFeedItem[]
}

function startOfDay(at: number): number {
  const day = new Date(at)
  day.setHours(0, 0, 0, 0)
  return day.getTime()
}

/** The commits and reverts newest first, one group per day. A Schedule
 *  entry is not a change of a file, so the view drops it. */
export function groupChanges(
  items: readonly MemoryFeedItem[],
  now: number = Date.now(),
): ChangeGroup[] {
  const today = startOfDay(now)
  const yesterday = startOfDay(today - 1)
  const groups: ChangeGroup[] = []
  const changes = items
    .filter((item) => item.kind !== 'schedule')
    .sort((left, right) => right.created_at - left.created_at)
  for (const item of changes) {
    const day = startOfDay(item.created_at)
    const label =
      day === today
        ? 'Today'
        : day === yesterday
          ? 'Yesterday'
          : new Date(item.created_at).toLocaleDateString('en-US', { month: 'short', day: 'numeric' })
    const last = groups.at(-1)
    if (last?.label === label) last.items.push(item)
    else groups.push({ label, items: [item] })
  }
  return groups
}
