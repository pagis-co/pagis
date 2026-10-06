// `/runs/:runId`: one run as a timeline.
//
// The trigger opens it, each tool call is a step card that says what it
// asked for and what it gave back, and the footer carries the tokens
// and the time. The raw event stays one disclosure away, so the page
// reads in words and still holds every fact.

import { ArrowLeft, ChevronDown, ChevronRight } from 'lucide-react'
import { useEffect, useState } from 'react'

import {
  fetchArtifactBlob,
  type ApiClient,
  type MemoryFeedItem,
  type RunEventDto,
} from '../../api/client'
import { Avatar, Badge, Button } from '../../primitives'
import {
  useAgents,
  useChannels,
  useMemoryFeed,
  useRetryReview,
  useRevertCommit,
  useRunTranscript,
} from '../../queries'
import {
  failureText,
  modelRequests,
  runDuration,
  runStateBadge,
  runSteps,
  stepDuration,
  triggerSentence,
  type RunStep,
} from './runs'

import './runs.css'

function Screenshot({ artifactId }: { artifactId: string }) {
  const [url, setUrl] = useState<string | null>(null)
  useEffect(() => {
    let active = true
    let objectUrl: string | null = null
    void fetchArtifactBlob(artifactId).then((blob) => {
      if (!active) return
      objectUrl = URL.createObjectURL(blob)
      setUrl(objectUrl)
    })
    return () => {
      active = false
      if (objectUrl !== null) URL.revokeObjectURL(objectUrl)
    }
  }, [artifactId])
  return url === null ? null : (
    <img className="runs-shot" src={url} alt={`Run screenshot ${artifactId}`} />
  )
}

function screenshotsOf(event: RunEventDto | null): string[] {
  if (event === null) return []
  const payload = event.payload as { artifact_ids?: unknown } | null
  const ids = payload === null ? undefined : payload.artifact_ids
  return Array.isArray(ids) ? ids.filter((id): id is string => typeof id === 'string') : []
}

function StepCard({ step, index }: { step: RunStep; index: number }) {
  const [open, setOpen] = useState(false)
  const shots = screenshotsOf(step.completed)
  return (
    <article className="runs-step" data-testid="run-step">
      <header className="runs-step-head">
        <span className="runs-step-index">{index + 1}</span>
        <strong className="runs-step-name">{step.name}</strong>
        <span className="runs-step-duration">{stepDuration(step)}</span>
      </header>
      <p className="runs-step-line">{step.args}</p>
      <p className={step.failed ? 'runs-step-line runs-step-failed' : 'runs-step-line'}>
        {step.result}
      </p>
      <Button
        size="sm"
        variant="ghost"
        className="runs-step-raw-toggle"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {open ? <ChevronDown size={14} aria-hidden /> : <ChevronRight size={14} aria-hidden />}
        Show raw event
      </Button>
      {open && (
        <>
          <pre className="runs-raw">{JSON.stringify(step.event.payload, null, 2)}</pre>
          {step.completed !== null && (
            <pre className="runs-raw">
              {JSON.stringify(step.completed.payload, null, 2)}
            </pre>
          )}
          {shots.map((artifactId) => (
            <Screenshot key={artifactId} artifactId={artifactId} />
          ))}
        </>
      )}
    </article>
  )
}

function payloadOf(event: RunEventDto): Record<string, unknown> {
  return event.payload !== null && typeof event.payload === 'object'
    ? (event.payload as Record<string, unknown>)
    : {}
}

function ReviewPending({ event }: { event: RunEventDto }) {
  const payload = payloadOf(event)
  const subject = typeof payload.subject === 'string' ? payload.subject : 'this subject'
  const urgency = payload.urgency === 'urgent' ? 'Urgent review' : 'Deferred review'
  return (
    <article className="runs-learning-event">
      <strong>Memory review queued</strong>
      <p>{subject}</p>
      <span>{urgency}. The reply is complete while this work waits.</span>
    </article>
  )
}

function Compacted({ event }: { event: RunEventDto }) {
  const payload = payloadOf(event)
  const before = payload.before_estimated_input_tokens
  const after = payload.after_estimated_input_tokens
  return (
    <article className="runs-learning-event">
      <strong>Conversation prepared</strong>
      <p>The sprite shortened older context so it could continue this reply.</p>
      {typeof before === 'number' && typeof after === 'number' && (
        <span>{before.toLocaleString()} → {after.toLocaleString()} estimated input tokens</span>
      )}
    </article>
  )
}

function sourceRange(payload: Record<string, unknown>): string {
  const range = payload.source_range
  if (range === null || typeof range !== 'object') return 'Source: this Run'
  const source = range as Record<string, unknown>
  const after = typeof source.after_exclusive === 'string' ? source.after_exclusive : 'start'
  const through = typeof source.through_inclusive === 'string' ? source.through_inclusive : 'unknown'
  return `Source: after ${after} through ${through}`
}

function MemoryChanged({
  api,
  event,
  item,
  revision,
  reverted,
}: {
  api: ApiClient
  event: RunEventDto
  item: MemoryFeedItem
  revision: string | null
  reverted: boolean
}) {
  const payload = payloadOf(event)
  const revert = useRevertCommit(api)
  const sha = item.sha
  return (
    <article className="runs-learning-event" role="region" aria-label="Memory changed">
      <strong>Memory changed</strong>
      <p>{item.message}</p>
      <span>{sourceRange(payload)}</span>
      {item.files.length > 0 && <span>{item.files.join(', ')}</span>}
      {reverted ? (
        <span>Reverted</span>
      ) : (
        <Button
          size="sm"
          variant="ghost"
          disabled={revision === null || revert.isPending}
          onClick={() => {
            if (revision !== null) revert.mutate({ sha, expectedRevision: revision })
          }}
        >
          Revert
        </Button>
      )}
      {revert.isError && <p role="alert">{revert.error.message}</p>}
    </article>
  )
}

export function RunTimeline({
  api,
  runId,
  onBack,
}: {
  api: ApiClient
  runId: string
  onBack: () => void
}) {
  const transcript = useRunTranscript(api, runId)
  const agents = useAgents(api)
  const channels = useChannels(api)
  const memory = useMemoryFeed(api)
  const retry = useRetryReview(api)

  if (transcript.data === undefined) {
    return (
      <div className="runs-panel">
        <Button variant="ghost" size="sm" className="runs-back" onClick={onBack}>
          <ArrowLeft size={14} aria-hidden />
          All runs
        </Button>
        <p className="runs-empty">Reading the run…</p>
      </div>
    )
  }

  const { run, usage, events } = transcript.data
  const agentName =
    (agents.data ?? []).find((agent) => agent.id === run.agent_id)?.name ?? 'A sprite'
  const channelName =
    run.channel_id == null
      ? null
      : (channels.data ?? []).find((channel) => channel.id === run.channel_id)?.title ??
        null
  const badge = runStateBadge(run.state)
  const failure = failureText(run)
  const steps = runSteps(events)
  const requests = modelRequests(events)
  const learningEvents = events.filter((event) =>
    ['memory.review_pending', 'context.compacted', 'memory.committed'].includes(event.event_type),
  )
  const reverted = new Set(
    (memory.data?.items ?? [])
      .filter((item) => item.kind === 'reverted' && item.reverted_sha != null)
      .map((item) => item.reverted_sha as string),
  )

  return (
    <div className="runs-panel">
      <Button variant="ghost" size="sm" className="runs-back" onClick={onBack}>
        <ArrowLeft size={14} aria-hidden />
        All runs
      </Button>

      <header className="runs-run-head">
        <Avatar appearance={(agents.data ?? []).find((agent) => agent.id === run.agent_id)?.avatar} id={run.agent_id} name={agentName} size="md" />
        <div>
          <h2>{agentName}</h2>
          <p className="runs-run-trigger">
            Started by {triggerSentence(run, channelName)}
          </p>
        </div>
        <Badge tone={badge.tone}>{badge.label}</Badge>
      </header>

      <section className="runs-steps" aria-label="Steps">
        {steps.length === 0 && <p className="runs-empty">This run called no tool.</p>}
        {steps.map((step, index) => (
          <StepCard key={step.event.id} step={step} index={index} />
        ))}
      </section>

      {requests.length > 0 && (
        <section className="runs-requests" aria-label="Model requests">
          <h3>Model requests</h3>
          {requests.map((request) => (
            <article key={request.event.id} className="runs-request" data-testid="model-request">
              <div className="runs-request-head">
                <strong>{request.label}</strong>
                {request.failed && <Badge tone="failed">Failed</Badge>}
              </div>
              <span>{request.summary}</span>
              {/* The error of the Run reads once, in the footer. */}
              {request.error !== null && request.error !== run.error && (
                <p className="runs-request-error">{request.error}</p>
              )}
            </article>
          ))}
        </section>
      )}

      {learningEvents.length > 0 && (
        <section className="runs-learning" aria-label="Conversation and memory">
          <h3>Conversation and memory</h3>
          {learningEvents.map((event) => {
            if (event.event_type === 'memory.review_pending') {
              return <ReviewPending key={event.id} event={event} />
            }
            if (event.event_type === 'context.compacted') {
              return <Compacted key={event.id} event={event} />
            }
            const sha = payloadOf(event).sha
            const item = (memory.data?.items ?? []).find(
              (candidate) => candidate.kind === 'committed' && candidate.sha === sha,
            )
            if (item === undefined) return null
            return (
              <MemoryChanged
                key={event.id}
                api={api}
                event={event}
                item={item}
                revision={memory.data?.revision ?? null}
                reverted={typeof sha === 'string' && reverted.has(sha)}
              />
            )
          })}
        </section>
      )}

      <footer className="runs-footer">
        {failure !== null && (
          <div className="runs-run-failure">
            <p>{failure}</p>
            {run.error != null && <p className="runs-run-error">{run.error}</p>}
          </div>
        )}
        {run.trigger_kind === 'review' && run.state === 'failed' && (
          <span className="runs-review-retry">
            <Button
              size="sm"
              variant="ghost"
              disabled={retry.isPending || retry.isSuccess}
              onClick={() => retry.mutate(run.id)}
            >
              Retry memory review
            </Button>
            {retry.isSuccess && <span role="status">Memory review queued again</span>}
            {retry.isError && <span role="alert">{retry.error.message}</span>}
          </span>
        )}
        <span>
          {usage.input_tokens} tokens in · {usage.output_tokens} tokens out
        </span>
        <span>{runDuration(run)}</span>
      </footer>
    </div>
  )
}
