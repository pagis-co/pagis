// One change of a Coding Session as a unified diff: a head with the
// path, then each hunk with the removed lines struck and the added
// lines marked. The diff is unified, so it reads on a phone. It shows no
// line numbers, because the harness sends no position in the file.

import { useMemo, useState } from 'react'

import { Badge, Button } from '../../primitives'
import { fileChange } from './diff'
import type { DiffContent } from './transcript'

/** A longer change shows this many lines until the reader opens it. */
const FOLD_LINES = 80

export function DiffView({ content }: { content: DiffContent }) {
  const { path, oldText, newText, truncated } = content
  const change = useMemo(
    () => fileChange({ type: 'diff', path, oldText, newText, truncated }),
    [path, oldText, newText, truncated],
  )
  const [open, setOpen] = useState(false)

  const head = (
    <header className="coding-diff-head">
      <span className="coding-mono">{path}</span>
      {!change.tooLarge && change.created && <Badge>New file</Badge>}
      {!change.tooLarge && change.deleted && <Badge>Deleted</Badge>}
    </header>
  )
  if (change.tooLarge) {
    return (
      <div className="coding-diff">
        {head}
        <p className="coding-diff-note">This change is too large to show here.</p>
      </div>
    )
  }

  const total = change.hunks.reduce((sum, hunk) => sum + hunk.length, 0)
  let room = open ? total : FOLD_LINES
  const shown = change.hunks.flatMap((hunk) => {
    if (room <= 0) return []
    const lines = hunk.slice(0, room)
    room -= lines.length
    return [lines]
  })

  return (
    <div className="coding-diff">
      {head}
      <div className="coding-diff-body">
        {shown.map((hunk, index) => (
          <div key={index} className="coding-diff-hunk">
            {hunk.map((line, lineIndex) =>
              line.kind === 'removed' ? (
                <del key={lineIndex} className="coding-diff-line">
                  {line.text}
                </del>
              ) : line.kind === 'added' ? (
                <ins key={lineIndex} className="coding-diff-line">
                  {line.text}
                </ins>
              ) : (
                <span key={lineIndex} className="coding-diff-line">
                  {line.text}
                </span>
              ),
            )}
          </div>
        ))}
      </div>
      {!open && total > FOLD_LINES && (
        <Button
          size="sm"
          variant="ghost"
          className="coding-disclosure"
          onClick={() => setOpen(true)}
        >
          Show all {total} lines
        </Button>
      )}
    </div>
  )
}
