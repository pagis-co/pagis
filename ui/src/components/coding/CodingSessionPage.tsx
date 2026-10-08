// `/coding/:sessionId`: one Coding Session (ADR-0033).
//
// The head says who runs which coding harness, where, and how far it
// is. The plan is a checklist, and the transcript shows the messages,
// the tool calls and one line for each ask. A tool call draws each of
// its changes as a diff, and "Changed files" is the index to them: a
// row scrolls to the last change of its file. Everything that the harness
// writes is foreign text: a message draws only through `Prose`, and
// each other text draws as plain text.
//
// A frame of the session invalidates the record and the transcript,
// and the page reads them again (`AppShell`). The back control goes to
// the Coding place, so at 760 px and below the list and the page are the
// two steps of one master-detail body (ADR-0022).

import { Link } from '@tanstack/react-router'
import {
  ArrowLeft,
  ChevronDown,
  ChevronRight,
  Circle,
  CircleCheck,
  CircleDot,
  MessageCircleQuestion,
  ShieldCheck,
  SquareTerminal,
} from 'lucide-react'
import { useState, type ReactNode } from 'react'

import type { AgentDto, ApiClient, CodingSessionDto } from '../../api/client'
import { Prose } from '../../prose'
import { Avatar, Badge, Button } from '../../primitives'
import { errorCode, useAgents, useCodingSession, useCodingSessionEvents } from '../../queries'
import { PageState } from '../PageState'
import { changedFiles, type ChangedFile } from './diff'
import { DiffView } from './DiffView'
import { StopCodingSession } from './StopCodingSession'
import { foldTranscript, type PlanEntry, type TranscriptItem } from './transcript'
import {
  answerText,
  approvalModeBadge,
  changeCountText,
  decisionText,
  lineCountText,
  planStatusWord,
  sessionSettled,
  sessionStateBadge,
  toolKindWord,
  toolStatusBadge,
  turnEndText,
  usageText,
  waitsForText,
} from './words'

import './coding.css'

/** The mark of a row that the daemon cut to fit its cap. */
function Cut({ truncated }: { truncated: boolean }) {
  return truncated ? <span className="coding-cut">Cut at 16 KB</span> : null
}

function Head({
  api,
  session,
  agent,
  spriteName,
}: {
  api: ApiClient
  session: CodingSessionDto
  agent: AgentDto | undefined
  spriteName: string
}) {
  const state = sessionStateBadge(session.state)
  const mode = approvalModeBadge(session.approval_mode, spriteName)
  const usage = usageText(session.usage)
  // A session in the Agent's Computer has no Host name.
  const machine = session.machine_name ?? 'Computer'
  return (
    <header className="coding-head">
      <div className="coding-head-top">
        <Avatar appearance={agent?.avatar} id={session.agent_id} name={spriteName} size="md" />
        <div className="coding-head-what">
          <h2>{session.title}</h2>
          <p className="coding-head-who">
            <span>{spriteName}</span>
            <span aria-hidden> · </span>
            <span>{session.harness_name}</span>
          </p>
        </div>
        <div className="coding-head-badges">
          <Badge tone={state.tone}>{state.label}</Badge>
          <Badge tone={mode.tone}>{mode.label}</Badge>
        </div>
        {!sessionSettled(session.state) && <StopCodingSession api={api} sessionId={session.id} />}
      </div>
      <dl className="coding-facts">
        <div>
          <dt>Machine</dt>
          <dd>{machine}</dd>
        </div>
        <div>
          <dt>Directory</dt>
          <dd className="coding-mono">{session.directory}</dd>
        </div>
        {session.worktree_branch != null && (
          <div>
            <dt>Branch</dt>
            <dd className="coding-mono">{session.worktree_branch}</dd>
          </div>
        )}
        {usage !== null && (
          <div>
            <dt>Usage</dt>
            <dd>{usage}</dd>
          </div>
        )}
      </dl>
      <Link
        to="/c/$channelId/t/$messageId"
        params={{ channelId: session.channel_id, messageId: session.root_message_id }}
        className="coding-thread-link"
      >
        Open the conversation
      </Link>
    </header>
  )
}

const PLAN_ICON = {
  pending: Circle,
  in_progress: CircleDot,
  completed: CircleCheck,
} as const

function Plan({ entries }: { entries: PlanEntry[] }) {
  return (
    <section className="coding-plan" aria-label="Plan">
      <h3>Plan</h3>
      <ul>
        {entries.map((entry, index) => {
          const Icon = PLAN_ICON[entry.status as keyof typeof PLAN_ICON] ?? Circle
          return (
            <li key={index} className={`coding-plan-entry coding-plan-${entry.status}`}>
              <Icon size={16} aria-hidden />
              <span className="coding-plan-content">{entry.content}</span>
              {entry.priority === 'high' && <Badge tone="accent">High priority</Badge>}
              <span className="coding-plan-status">{planStatusWord(entry.status)}</span>
            </li>
          )
        })}
      </ul>
    </section>
  )
}

/** The id of the card of a tool call, so a changed file can scroll to
 *  it. */
function toolAnchor(toolCallId: string): string {
  return `coding-tool-${toolCallId}`
}

function ChangedFiles({ files }: { files: ChangedFile[] }) {
  return (
    <section className="coding-files" aria-label="Changed files">
      <h3>Changed files</h3>
      <ul>
        {files.map((file) => (
          <li key={file.path}>
            <Button
              variant="ghost"
              className="coding-file"
              onClick={() => document.getElementById(toolAnchor(file.toolCallId))?.scrollIntoView()}
            >
              <span className="coding-file-path coding-mono">{file.path}</span>
              <span className="coding-file-lines">{lineCountText(file.added, file.removed)}</span>
              <span className="coding-file-changes">{changeCountText(file.changes)}</span>
            </Button>
          </li>
        ))}
      </ul>
    </section>
  )
}

type Item<K extends TranscriptItem['kind']> = Extract<TranscriptItem, { kind: K }>

function Message({ item, author }: { item: Item<'message'>; author: string }) {
  return (
    <article className={`coding-message coding-message-${item.from}`}>
      <span className="coding-author">{author}</span>
      <div className="coding-prose prose">
        <Prose>{item.text}</Prose>
      </div>
      <Cut truncated={item.truncated} />
    </article>
  )
}

/** The reasoning of the harness, closed until the reader opens it. */
function Thought({ item }: { item: Item<'thought'> }) {
  const [open, setOpen] = useState(false)
  return (
    <div className="coding-thought">
      <Button
        size="sm"
        variant="ghost"
        className="coding-disclosure"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {open ? <ChevronDown size={14} aria-hidden /> : <ChevronRight size={14} aria-hidden />}
        Thinking
      </Button>
      {open && (
        <div className="coding-prose prose">
          <Prose>{item.text}</Prose>
        </div>
      )}
      <Cut truncated={item.truncated} />
    </div>
  )
}

function ToolCall({ item }: { item: Item<'tool'> }) {
  const [open, setOpen] = useState(false)
  const status = toolStatusBadge(item.status)
  const title = item.title === '' ? toolKindWord(item.toolKind) : item.title
  return (
    <article id={toolAnchor(item.toolCallId)} className="coding-tool" aria-label={title}>
      <header className="coding-tool-head">
        <span className="coding-tool-kind">{toolKindWord(item.toolKind)}</span>
        <strong className="coding-tool-title">{title}</strong>
        <Badge tone={status.tone}>{status.label}</Badge>
      </header>
      {item.locations.length > 0 && (
        <ul className="coding-tool-locations">
          {item.locations.map((location, index) => (
            <li key={index} className="coding-mono">
              {location.line === undefined ? location.path : `${location.path}:${location.line}`}
            </li>
          ))}
        </ul>
      )}
      {item.content.map((content, index) =>
        content.type === 'text' ? (
          <pre key={index} className="coding-tool-text">
            {content.text}
          </pre>
        ) : (
          <DiffView key={index} content={content} />
        ),
      )}
      {item.rawInput !== undefined && (
        <>
          <Button
            size="sm"
            variant="ghost"
            className="coding-disclosure"
            aria-expanded={open}
            onClick={() => setOpen(!open)}
          >
            {open ? <ChevronDown size={14} aria-hidden /> : <ChevronRight size={14} aria-hidden />}
            Show the input
          </Button>
          {open && <pre className="coding-raw">{JSON.stringify(item.rawInput, null, 2)}</pre>}
        </>
      )}
      <Cut truncated={item.truncated} />
    </article>
  )
}

function Permission({ item, spriteName }: { item: Item<'permission'>; spriteName: string }) {
  const outcome =
    item.decision === null
      ? waitsForText(item.waitsFor, spriteName)
      : decisionText(item.decision, item.decider, spriteName)
  return (
    <p className="coding-line">
      <ShieldCheck size={14} aria-hidden />
      <span className="coding-line-what">{item.title ?? toolKindWord(item.toolKind)}</span>
      <span className="coding-line-outcome">{outcome}</span>
      <Cut truncated={item.truncated} />
    </p>
  )
}

function Question({ item, spriteName }: { item: Item<'question'>; spriteName: string }) {
  const outcome =
    item.answer === null ? waitsForText(item.waitsFor, spriteName) : answerText(item.answer)
  return (
    <p className="coding-line">
      <MessageCircleQuestion size={14} aria-hidden />
      <span className="coding-line-what">{item.message}</span>
      <span className="coding-line-outcome">{outcome}</span>
      <Cut truncated={item.truncated} />
    </p>
  )
}

function TranscriptView({
  items,
  spriteName,
  harnessName,
}: {
  items: TranscriptItem[]
  spriteName: string
  harnessName: string
}) {
  return (
    <section className="coding-transcript" aria-label="Transcript">
      {items.length === 0 && <p className="coding-empty">The coding harness has written nothing.</p>}
      {items.map((item) => {
        switch (item.kind) {
          case 'message':
            return (
              <Message
                key={item.seq}
                item={item}
                author={item.from === 'sprite' ? spriteName : harnessName}
              />
            )
          case 'thought':
            return <Thought key={item.seq} item={item} />
          case 'tool':
            return <ToolCall key={item.seq} item={item} />
          case 'permission':
            return <Permission key={item.seq} item={item} spriteName={spriteName} />
          case 'question':
            return <Question key={item.seq} item={item} spriteName={spriteName} />
          case 'turn_end':
            return (
              <p key={item.seq} className="coding-turn-end">
                <SquareTerminal size={14} aria-hidden />
                {turnEndText(item.stopReason)}
              </p>
            )
        }
      })}
    </section>
  )
}

/** The page in each of its states, with the back control to the
 *  Coding place on top. */
function Page({ onBack, children }: { onBack: () => void; children: ReactNode }) {
  return (
    <div className="coding-page">
      <Button variant="ghost" size="sm" className="coding-back" onClick={onBack}>
        <ArrowLeft size={14} aria-hidden />
        All coding sessions
      </Button>
      {children}
    </div>
  )
}

export function CodingSessionPage({
  api,
  sessionId,
  onBack,
}: {
  api: ApiClient
  sessionId: string
  onBack: () => void
}) {
  const session = useCodingSession(api, sessionId)
  const events = useCodingSessionEvents(api, sessionId)
  const agents = useAgents(api)

  if (errorCode(session.error) === 'not_found') {
    return (
      <Page onBack={onBack}>
        <PageState icon={SquareTerminal} title="That coding session is not on record." />
      </Page>
    )
  }
  if (session.isError) {
    return (
      <Page onBack={onBack}>
        <PageState
          icon={SquareTerminal}
          title="The coding session did not load."
          onRetry={() => void session.refetch()}
        />
      </Page>
    )
  }
  if (session.data === undefined) {
    return (
      <Page onBack={onBack}>
        <p className="coding-empty">Reading the coding session…</p>
      </Page>
    )
  }

  const record = session.data
  const agent = (agents.data ?? []).find((candidate) => candidate.id === record.agent_id)
  const spriteName = agent?.name ?? 'A sprite'
  const rows = (events.data?.pages ?? []).flatMap((page) => page.items)
  const { items, plan } = foldTranscript(rows)
  const files = changedFiles(items)

  return (
    <Page onBack={onBack}>
      <Head api={api} session={record} agent={agent} spriteName={spriteName} />
      {plan !== null && plan.length > 0 && <Plan entries={plan} />}
      {files.length > 0 && <ChangedFiles files={files} />}
      {events.data === undefined ? (
        <p className="coding-empty">Reading the transcript…</p>
      ) : (
        <TranscriptView items={items} spriteName={spriteName} harnessName={record.harness_name} />
      )}
    </Page>
  )
}
