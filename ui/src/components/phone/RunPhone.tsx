import { useNavigate, useSearch } from '@tanstack/react-router'
import { AlertCircle, Check, Monitor, RotateCcw, X } from 'lucide-react'
import type { ApiClient } from '../../api/client'
import { Avatar, Badge, Button, Frame, Row, SectionLabel } from '../../primitives'
import {
  errorMessage,
  useAgents,
  useChannels,
  useRetryReview,
  useRunSteps,
  useRunTranscript,
} from '../../queries'
import { directMessageChannel } from '../AskAnAgent'
import {
  failureNextStep,
  failureText,
  runDuration,
  runStateBadge,
  runSteps,
  triggerSentence,
} from '../runs/runs'
import { phoneParent } from '../../mobileShell'
import { formatClock } from '../../timeline'
import { NavBar } from './TopBar'

export function RunPhone({ api, runId }: { api: ApiClient; runId: string }) {
  const navigate = useNavigate()
  const { from } = useSearch({ strict: false })
  const transcript = useRunTranscript(api, runId)
  const steps = useRunSteps(api, runId)
  const agents = useAgents(api)
  const channels = useChannels(api)
  const retry = useRetryReview(api)
  const parent = phoneParent(`/runs/${runId}`, from)
  const back = () => void navigate({ href: parent.path })
  const content = transcript.data
  if (!content)
    return (
      <>
        <NavBar back={{ label: parent.label, onBack: back }} />
        <p className="phone-content" role={transcript.isError ? 'alert' : 'status'}>
          {transcript.isError ? 'Could not read the run.' : 'Reading the run…'}
        </p>
      </>
    )
  const { run, events } = content
  const agent = agents.data?.find((row) => row.id === run.agent_id)
  const badge = runStateBadge(run.state)
  const failure = failureText(run)
  const channelId = directMessageChannel(channels.data ?? [], run.agent_id)
  const details = runSteps(events)
  return (
    <>
      <NavBar back={{ label: parent.label, onBack: back }} />
      <div className="phone-content">
        <header className="phone-resource-head">
          <Avatar
            id={run.agent_id}
            name={agent?.name ?? 'Sprite'}
            appearance={agent?.avatar}
            size="xl"
          />
          <div className="phone-row-copy">
            <h1 className="phone-heading">{run.title}</h1>
            <div>
              <Badge tone={badge.tone}>{badge.label}</Badge>
            </div>
            <p className="phone-hint">
              {agent?.name} · Started by {triggerSentence(run, null)} ·{' '}
              {formatClock(run.created_at)}
            </p>
          </div>
        </header>
        {failure && (
          <div className="phone-failure">
            <AlertCircle size={20} aria-hidden />
            <div>
              <strong>{failure}.</strong>
              {run.error && <p>{run.error}</p>}
              <p>{failureNextStep(run)}</p>
            </div>
          </div>
        )}
        <div className="phone-actions">
          <Button
            variant="primary"
            size="lg"
            onClick={() =>
              void navigate({
                to: '/sprites/$agentId/desk',
                params: { agentId: run.agent_id },
                search: { from: `/runs/${run.id}` },
              })
            }
          >
            <Monitor size={20} aria-hidden />
            Open the desk
          </Button>
          {run.trigger_kind === 'review' && run.state === 'failed' && (
            <Button
              size="lg"
              disabled={retry.isPending || retry.isSuccess}
              onClick={() => retry.mutate(run.id)}
            >
              <RotateCcw size={20} aria-hidden />
              Try again
            </Button>
          )}
        </div>
        {retry.isSuccess && (
          <p role="status" className="phone-hint">
            Memory review queued again
          </p>
        )}
        {retry.isError && (
          <p role="alert" className="phone-hint">
            {errorMessage(retry.error, 'The memory review could not start again.')}
          </p>
        )}
        <section className="phone-section">
          <div className="phone-section-head">
            <SectionLabel>Steps</SectionLabel>
            <span className="phone-hint">{runDuration(run)}</span>
          </div>
          <Frame>
            {(steps.data?.steps ?? []).map((step) => {
              const detail = details[step.index - 1]
              const failed =
                detail?.failed ??
                (run.state === 'failed' && step.index === steps.data?.steps.length)
              return (
                <details className="phone-step" key={step.index}>
                  <summary>
                    <span className="phone-step-head">
                      <span className={failed ? 'phone-step-failed' : 'phone-step-done'}>
                        {failed ? <X size={14} aria-hidden /> : <Check size={14} aria-hidden />}
                      </span>
                      <span>{step.label}</span>
                      <span className="phone-row-time">
                        {formatClock(step.started_at)}
                      </span>
                    </span>
                  </summary>
                  {detail && (
                    <div className="phone-step-detail">
                      <p>{detail.args}</p>
                      <p>{detail.result}</p>
                      <pre className="approval-phone-command">
                        {JSON.stringify(detail.event.payload, null, 2)}
                      </pre>
                    </div>
                  )}
                </details>
              )
            })}
            {steps.isPending && <Row>Reading steps…</Row>}
            {steps.isError && <Row>Could not read the steps.</Row>}
            {steps.data?.steps.length === 0 && <Row>This run called no tool.</Row>}
          </Frame>
        </section>
        {channelId && (
          <Frame>
            <Row
              chevron
              onClick={() => void navigate({ to: '/c/$channelId', params: { channelId } })}
            >
              <Avatar
                id={run.agent_id}
                name={agent?.name ?? 'Sprite'}
                appearance={agent?.avatar}
                size="sm"
              />
              Ask {agent?.name ?? 'the sprite'} about this run
            </Row>
          </Frame>
        )}
      </div>
    </>
  )
}
