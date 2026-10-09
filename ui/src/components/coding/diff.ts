// The changes of a Coding Session as the session page draws them
// (ADR-0033).
//
// A harness sends a change as the old and the new text of one file,
// with no position in the file. The Product App computes the hunks from
// the two texts with jsdiff, so the view shows no line numbers. A diff
// in a payload that the daemon cut can hold an incomplete text, and an
// incomplete text gives a false diff, so such a change has no hunks.

import { structuredPatch } from 'diff'

import type { DiffContent, TranscriptItem } from './transcript'

/** The count of unchanged lines around each change. */
const CONTEXT_LINES = 3

export interface DiffLine {
  kind: 'same' | 'removed' | 'added'
  text: string
}

/** One changed range of a file and its unchanged lines around it. */
export type Hunk = DiffLine[]

export type FileChange =
  | { path: string; tooLarge: true }
  | {
      path: string
      tooLarge: false
      /** The file did not exist before the change. */
      created: boolean
      /** The change emptied the file. */
      deleted: boolean
      hunks: Hunk[]
      added: number
      removed: number
    }

const LINE_KIND: Record<string, DiffLine['kind']> = {
  ' ': 'same',
  '-': 'removed',
  '+': 'added',
}

/** The hunks and the counts of one change. */
export function fileChange(content: DiffContent): FileChange {
  const { path, oldText, newText } = content
  if (content.truncated) return { path, tooLarge: true }

  const patch = structuredPatch(path, path, oldText ?? '', newText, undefined, undefined, {
    context: CONTEXT_LINES,
  })
  // A line that starts with `\` says that the text has no last newline.
  // It is not a line of the file, so it does not show.
  const hunks = patch.hunks.map((hunk) =>
    hunk.lines.flatMap((line): DiffLine[] => {
      const kind = LINE_KIND[line[0]]
      return kind === undefined ? [] : [{ kind, text: line.slice(1) }]
    }),
  )
  const lines = hunks.flat()
  return {
    path,
    tooLarge: false,
    created: oldText === null,
    deleted: oldText !== null && newText === '',
    hunks,
    added: lines.filter((line) => line.kind === 'added').length,
    removed: lines.filter((line) => line.kind === 'removed').length,
  }
}

/** All the changes of one file in a session. */
export interface ChangedFile {
  path: string
  /** The count of changes. */
  changes: number
  /** The added and the removed lines over all changes. A change that
   *  is too large to show adds no lines. */
  added: number
  removed: number
  /** The tool call of the last change. */
  toolCallId: string
}

/** One entry for each file that the session changed, in the order of
 *  its first change. */
export function changedFiles(items: readonly TranscriptItem[]): ChangedFile[] {
  const files = new Map<string, ChangedFile>()
  for (const item of items) {
    if (item.kind !== 'tool') continue
    for (const content of item.content) {
      if (content.type !== 'diff') continue
      const change = fileChange(content)
      const file = files.get(change.path) ?? {
        path: change.path,
        changes: 0,
        added: 0,
        removed: 0,
        toolCallId: item.toolCallId,
      }
      file.changes += 1
      if (!change.tooLarge) {
        file.added += change.added
        file.removed += change.removed
      }
      file.toolCallId = item.toolCallId
      files.set(change.path, file)
    }
  }
  return [...files.values()]
}
