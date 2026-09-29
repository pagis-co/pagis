// One row of the Changes view: the Agent and the sentence, the
// time, the source and the pages, then Open the Run and Revert. The
// sentence opens the row to the before and after of the commit.

import { useState } from 'react'

import type { ApiClient, MemoryFeedItem } from '../../api/client'
import { Avatar, Badge, Button, Row } from '../../primitives'
import { useRevertCommit } from '../../queries'
import { CommitDiff } from './CommitDiff'
import { authorName, changePaths, sentence, sourceLabel } from './changes'

/** The chips a row shows before it counts the rest. */
const CHIP_LIMIT = 3

export function ChangeRow({
  api,
  scope,
  item,
  reverted,
  revision,
  pageTitle,
  onOpenPage,
  onOpenRun,
}: {
  api: ApiClient
  scope: string
  item: MemoryFeedItem
  /** A revert, or a commit that a revert undid: the row has no actions. */
  reverted: boolean
  /** The memory revision the revert expects. */
  revision: string | null
  pageTitle: (path: string) => string
  onOpenPage: (path: string) => void
  onOpenRun: (runId: string) => void
}) {
  const [open, setOpen] = useState(false)
  const revert = useRevertCommit(api)
  const author = authorName(item)
  const paths = changePaths(item, scope)
  const hidden = paths.length - CHIP_LIMIT
  const time = new Date(item.created_at).toLocaleTimeString('en-GB', {
    hour: '2-digit',
    minute: '2-digit',
  })
  const meta = [time, sourceLabel(item)].filter((part) => part !== null).join(' · ')

  return (
    <Row role="listitem" className="memory-change">
      <div className="memory-change-head">
        <Avatar
          id={item.kind === 'reverted' ? 'you' : (item.agent_id ?? 'you')}
          name={author}
          size="sm"
        />
        <span className="memory-change-text">
          <Button
            variant="link"
            className="memory-change-sentence"
            aria-expanded={open}
            onClick={() => setOpen((current) => !current)}
          >
            <strong>{author}</strong> {sentence(item)}
          </Button>
          <span className="memory-change-meta">
            {meta}
            {paths.slice(0, CHIP_LIMIT).map((path) => (
              <Button
                key={path}
                variant="ghost"
                size="sm"
                className="memory-change-chip"
                onClick={() => onOpenPage(path)}
              >
                {pageTitle(path)}
              </Button>
            ))}
            {hidden > 0 && <Badge>+{hidden}</Badge>}
          </span>
        </span>
        {reverted ? (
          <Badge className="memory-change-actions">Reverted</Badge>
        ) : (
          <span className="memory-change-actions">
            {item.run_id != null && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => {
                  if (item.run_id != null) onOpenRun(item.run_id)
                }}
              >
                Open the Run
              </Button>
            )}
            <Button
              variant="ghost"
              size="sm"
              disabled={revert.isPending || revision === null}
              onClick={() => {
                if (revision !== null) revert.mutate({ sha: item.sha, expectedRevision: revision })
              }}
            >
              Revert
            </Button>
          </span>
        )}
      </div>
      {revert.isError && (
        <p role="alert" className="memory-empty">
          {revert.error.message}
        </p>
      )}
      {open && <CommitDiff api={api} scope={scope} sha={item.sha} pageTitle={pageTitle} />}
    </Row>
  )
}
