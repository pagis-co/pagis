// One memory page as the daemon holds it: the header with the
// scope and the actions, the path, and the body. A Subject Page shows
// its parts; any other page shows its Markdown.

import type { AgentDto, ApiClient, ChannelDto } from '../../api/client'
import { useMemoryFile } from '../../queries'
import { useComposerDraft } from '../../state/composerDraft'
import { threadScope } from '../../timeline'
import { Prose } from '../../prose'
import { Badge, Button } from '../../primitives'
import { directMessageChannel } from '../AskAnAgent'
import { ForgetPage } from './ForgetPage'
import { SubjectPageBody } from './SubjectPageBody'
import { pageProse, parseSubjectPage } from './subjectPage'

export function PageView({
  api,
  scope,
  path,
  owner,
  asker,
  channels,
  connectionName,
  onOpenChannel,
}: {
  api: ApiClient
  scope: string
  path: string
  /** The Agent of a private scope; `null` for Shared. */
  owner: AgentDto | null
  /** The Agent the Ask action talks to: the owner, else the Chief of Staff. */
  asker: AgentDto | null
  channels: ChannelDto[]
  connectionName: (connectionId: string) => string
  onOpenChannel: (channelId: string) => void
}) {
  const file = useMemoryFile(api, scope, path)
  const setDraft = useComposerDraft((state) => state.set)
  const subject = file.data === undefined ? null : parseSubjectPage(file.data.content)
  const title = file.data?.title ?? path
  const askChannel = asker === null ? null : directMessageChannel(channels, asker.id)
  const workspaceId = channels[0]?.workspace_id

  const ask = () => {
    if (askChannel === null) return
    const where = owner === null ? 'shared memory' : 'your memory'
    setDraft(
      threadScope(askChannel),
      `What do you know about ${title}? The page is ${path} in ${where}.`,
    )
    onOpenChannel(askChannel)
  }

  return (
    <article className="memory-article">
      <header className="memory-article-header">
        <h1>{title}</h1>
        {file.data?.kind != null && (
          <span className="memory-article-kind">{file.data.kind}</span>
        )}
        {owner === null ? (
          <Badge tone="working">Shared · every sprite</Badge>
        ) : (
          <Badge tone="accent">{owner.name} · private</Badge>
        )}
        <span className="memory-article-actions">
          {asker !== null && askChannel !== null && (
            <Button size="sm" onClick={ask}>
              Ask {asker.name} about it
            </Button>
          )}
          {subject !== null && subject.timeline.length > 0 && workspaceId !== undefined && (
            <ForgetPage
              api={api}
              workspaceId={workspaceId}
              title={title}
              timeline={subject.timeline}
            />
          )}
        </span>
      </header>
      <code className="memory-article-path">{path}</code>

      {file.isPending && <p className="memory-empty">Loading the page.</p>}
      {file.isError && (
        <p role="alert" className="memory-empty">
          Could not load the page.
        </p>
      )}
      {file.data !== undefined &&
        (subject === null ? (
          <div className="memory-prose prose">
            <Prose>{pageProse(file.data.content)}</Prose>
          </div>
        ) : (
          <SubjectPageBody
            page={subject}
            writer={owner?.name ?? 'The sprite'}
            connectionName={connectionName}
          />
        ))}
      {owner === null && (
        <p className="memory-note">
          A shared page has no Timeline: it is what sprites wrote, not what a source said.
        </p>
      )}
    </article>
  )
}
