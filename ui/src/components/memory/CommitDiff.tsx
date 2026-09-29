// The before and after of one commit: each changed range of
// each file in the scope, the removed lines struck on the left and the
// added lines on the right.

import type { ApiClient } from '../../api/client'
import { useCommitDiff } from '../../queries'

export function CommitDiff({
  api,
  scope,
  sha,
  pageTitle,
}: {
  api: ApiClient
  scope: string
  sha: string
  pageTitle: (path: string) => string
}) {
  const diff = useCommitDiff(api, sha)

  if (diff.isPending) return <p className="memory-empty">Loading the change.</p>
  if (diff.isError) {
    return (
      <p role="alert" className="memory-empty">
        Could not load the change.
      </p>
    )
  }
  const hunks = diff.data.files
    .filter((file) => file.scope === scope)
    .flatMap((file) =>
      file.hunks.map((hunk, index) => ({ key: `${file.path} ${index}`, path: file.path, hunk })),
    )
  if (hunks.length === 0) {
    return <p className="memory-empty">This change touched no file in this scope.</p>
  }

  return (
    <div className="memory-diff">
      {hunks.map(({ key, path, hunk }) => (
        <div key={key} className="memory-diff-pair">
          <div className="memory-diff-side">
            <div className="memory-diff-label">{pageTitle(path)} · before</div>
            {hunk.old_lines.map((line, index) => (
              <del key={index} className="memory-diff-line">
                {line}
              </del>
            ))}
          </div>
          <div className="memory-diff-side">
            <div className="memory-diff-label">after</div>
            {hunk.new_lines.map((line, index) => (
              <ins key={index} className="memory-diff-line">
                {line}
              </ins>
            ))}
          </div>
        </div>
      ))}
    </div>
  )
}
