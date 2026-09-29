// The learned line: after a reply whose Run wrote memory, one
// dashed line names what the Agent noted and the pages it touched.
// Show opens the pages as they read now; Undo reverts the commit.
// The line reads the memory feed, where each commit names the reply
// its Run settled.

import { Book } from 'lucide-react'
import { useState } from 'react'

import type { ApiClient, MemoryFeedItem } from '../api/client'
import { Button } from '../primitives'
import { useMemoryFeed, useMemoryFile, useRevertCommit } from '../queries'
import { memoryFileTarget } from './memoryFile'

import './LearnedLine.css'

/** The sentence Reflection wrote, as the words after "learned ·". */
function notedWords(message: string): string {
  const words = message.trim().replace(/\.$/, '')
  return words.charAt(0).toLowerCase() + words.slice(1)
}

function LearnedPage({
  api,
  scope,
  path,
  title,
}: {
  api: ApiClient
  scope: string
  path: string
  title: string
}) {
  const file = useMemoryFile(api, scope, path)
  return (
    <div className="learned-page">
      <div className="learned-page-name">{title}</div>
      {file.isPending ? (
        <p className="learned-page-state">Reading the page…</p>
      ) : file.isError ? (
        <p className="learned-page-state">
          This page is unavailable. It may be deleted or its source access has changed.
        </p>
      ) : (
        <pre className="learned-page-content">{file.data.content}</pre>
      )}
    </div>
  )
}

function LearnedCommit({
  api,
  item,
  authorName,
  revision,
  undone,
}: {
  api: ApiClient
  item: MemoryFeedItem
  authorName: string
  revision: string | null
  undone: boolean
}) {
  const revert = useRevertCommit(api)
  const [showing, setShowing] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // The daemon names each page by its title, never by a path or an id.
  const pages = item.files.map((file, index) => ({
    file,
    title: item.titles[index] ?? '',
    target: memoryFileTarget(item.agent_id, file),
  }))

  return (
    <div className="learned" data-testid="learned-line">
      <div className="learned-line">
        <Book size={14} aria-hidden focusable="false" className="learned-icon" />
        <span className="learned-words">
          {authorName} learned · {notedWords(item.message)} on{' '}
          {pages.map(({ file, title }, index) => (
            <span key={file}>
              {index > 0 && ', '}
              <Button variant="link" className="learned-page-link" onClick={() => setShowing(true)}>
                {title}
              </Button>
            </span>
          ))}
        </span>
        <span className="learned-acts">
          <Button
            size="sm"
            variant="ghost"
            aria-expanded={showing}
            onClick={() => setShowing((open) => !open)}
          >
            {showing ? 'Hide' : 'Show'}
          </Button>
          {undone ? (
            <span className="learned-undone">Undone</span>
          ) : (
            <Button
              size="sm"
              variant="ghost"
              disabled={revert.isPending || revision === null}
              onClick={() => {
                if (revision === null) return
                setError(null)
                revert.mutate(
                  { sha: item.sha, expectedRevision: revision },
                  { onError: (err) => setError(err.message) },
                )
              }}
            >
              Undo
            </Button>
          )}
        </span>
      </div>
      {error !== null && <p className="learned-error">{error}</p>}
      {showing &&
        pages.map(({ file, title, target }) =>
          target === null ? null : (
            <LearnedPage
              key={file}
              api={api}
              scope={target.scope}
              path={target.path}
              title={title}
            />
          ),
        )}
    </div>
  )
}

export function LearnedLine({
  api,
  messageId,
  authorName,
}: {
  api: ApiClient
  /** The reply the line follows. */
  messageId: string
  /** The Agent that wrote the reply. */
  authorName: string
}) {
  const feed = useMemoryFeed(api)
  const items = feed.data?.items ?? []
  const commits = items.filter(
    (item) => item.kind === 'committed' && item.message_id === messageId,
  )
  if (commits.length === 0) return null
  const undone = new Set(
    items
      .filter((item) => item.kind === 'reverted')
      .map((item) => item.reverted_sha),
  )
  return (
    <>
      {commits.map((item) => (
        <LearnedCommit
          key={item.id}
          api={api}
          item={item}
          authorName={authorName}
          revision={feed.data?.revision ?? null}
          undone={undone.has(item.sha)}
        />
      ))}
    </>
  )
}
