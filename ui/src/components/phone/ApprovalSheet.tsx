import { useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { FileText } from 'lucide-react'
import type { ApiClient } from '../../api/client'
import { Avatar, Badge, Button, Frame, Row, SectionLabel, Sheet, Switch } from '../../primitives'
import {
  errorMessage,
  useAgents,
  useChannels,
  useDecideRequest,
  useRequest,
  useRunTranscript,
  useWorkspace,
} from '../../queries'
import { alwaysAllowReach, alwaysAllowWords, type ApprovalPayload } from '../../blocks/ApprovalCard'
import { directMessageChannel } from '../AskAnAgent'
import { age } from '../home/HomePhone'
import { triggerSentence } from '../runs/runs'

interface Payload extends ApprovalPayload {
  action_title?: string
  body?: string
  arguments?: Record<string, unknown>
}

export function ApprovalSheet({
  api,
  requestId,
  onClose,
}: {
  api: ApiClient
  requestId: string
  onClose: () => void
}) {
  const request = useRequest(api, requestId)
  const decide = useDecideRequest(api, requestId)
  const agents = useAgents(api)
  const channels = useChannels(api)
  const workspace = useWorkspace(api)
  const transcript = useRunTranscript(api, request.data?.run_id ?? null)
  const navigate = useNavigate()
  const [always, setAlways] = useState(false)
  const row = request.data
  const agent = agents.data?.find((agent) => agent.id === row?.agent_id)
  const payload = row?.payload as Payload | undefined
  const args = payload?.arguments ?? {}
  const host =
    payload?.tool_name === 'host_shell' ||
    payload?.command != null ||
    row?.kind === 'harness_permission'
  const credential = row?.kind === 'credential_action'
  const mail = typeof args.subject === 'string' && args.to != null
  const pending = row?.state === 'pending'
  const alwaysWords = alwaysAllowWords(payload)
  const reach = alwaysAllowReach(row?.kind, payload)
  const run = transcript.data?.run
  const runChannel = channels.data?.find((channel) => channel.id === run?.channel_id)
  const reason = run
    ? `Started by ${triggerSentence(run, runChannel?.title ?? null)}.`
    : payload?.body
  const submit = (decision: 'approved' | 'denied') =>
    decide.mutate(
      decision === 'approved' && always ? { decision, scope: 'always' } : { decision },
      { onSuccess: onClose },
    )
  const label =
    row?.state === 'superseded'
      ? 'You replied instead'
      : row?.state === 'approved'
        ? 'Approved'
        : row?.state === 'denied'
          ? 'Denied'
          : 'Expired'
  return (
    <Sheet
      open
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
      title="Approval"
      showHeader={false}
      footer={
        pending ? (
          <>
            <div className="phone-actions">
              <Button size="lg" disabled={decide.isPending} onClick={() => submit('denied')}>
                Deny
              </Button>
              <Button
                size="lg"
                variant="primary"
                disabled={decide.isPending}
                onClick={() => submit('approved')}
              >
                Approve
              </Button>
            </div>
            <div className="approval-phone-reply">
              <Button
                variant="link"
                disabled={!row || !directMessageChannel(channels.data ?? [], row.agent_id)}
                onClick={() => {
                  const id = row ? directMessageChannel(channels.data ?? [], row.agent_id) : null
                  if (id) {
                    onClose()
                    void navigate({ to: '/c/$channelId', params: { channelId: id } }).then(() =>
                      document.querySelector<HTMLTextAreaElement>('.composer-input')?.focus(),
                    )
                  }
                }}
              >
                Reply to {agent?.name ?? 'the sprite'} instead
              </Button>
            </div>
          </>
        ) : undefined
      }
    >
      {row ? (
        <div className="phone-form">
          <div className="approval-phone-head">
            <Avatar
              id={row.agent_id}
              name={agent?.name ?? 'Sprite'}
              appearance={agent?.avatar}
              size="lg"
              presence={pending ? 'waiting' : 'none'}
            />
            <div className="phone-row-copy">
              <strong>{agent?.name ?? 'Sprite'} needs your approval</strong>
              <span className="phone-hint">
                {age(row.created_at) === 'Now' ? 'Just now' : `${age(row.created_at)} ago`}
                {workspace.data?.chief_of_staff_agent_id === row.agent_id && ' · Main sprite'}
              </span>
            </div>
          </div>
          <div>
            <h1 className="phone-heading">{payload?.action_title ?? 'Approval'}</h1>
            <p className="approval-phone-reason">
              {reason}{' '}
              {row.run_id && (
                <Button
                  variant="link"
                  className="phone-accent"
                  onClick={() => {
                    onClose()
                    void navigate({ to: '/runs/$runId', params: { runId: row.run_id! } })
                  }}
                >
                  Open run
                </Button>
              )}
            </p>
          </div>
          <section className="phone-section">
            <SectionLabel>
              {mail
                ? `The email ${agent?.name ?? 'the sprite'} will send`
                : host
                  ? 'The command'
                  : 'What will happen'}
            </SectionLabel>
            <Frame>
              {mail ? (
                <>
                  <Row>
                    <span className="approval-phone-field phone-hint">To</span>
                    <span>{Array.isArray(args.to) ? args.to.join(', ') : String(args.to)}</span>
                  </Row>
                  {['cc', 'bcc'].map(
                    (field) =>
                      Array.isArray(args[field]) &&
                      args[field].length > 0 && (
                        <Row key={field}>
                          <span className="approval-phone-field phone-hint">
                            {field === 'cc' ? 'Cc' : 'Bcc'}
                          </span>
                          <span>{(args[field] as string[]).join(', ')}</span>
                        </Row>
                      ),
                  )}
                  <Row>
                    <span className="approval-phone-field phone-hint">Subject</span>
                    <span>{String(args.subject)}</span>
                  </Row>
                  <Row>
                    <div className="phone-row-copy">
                      <p className="approval-phone-body">{String(args.body ?? '')}</p>
                      {Array.isArray(args.attachments) && args.attachments.length > 0 && (
                        <div className="phone-badges">
                          {args.attachments.map((attachment, index) => (
                            <Badge key={index}>
                              <FileText size={14} aria-hidden />
                              {typeof attachment === 'string'
                                ? attachment
                                : String(
                                    (attachment as Record<string, unknown>).name ??
                                      `Attachment ${index + 1}`,
                                  )}
                            </Badge>
                          ))}
                        </div>
                      )}
                    </div>
                  </Row>
                </>
              ) : host ? (
                <>
                  <Row>
                    <code className="approval-phone-command">
                      {String(args.command ?? payload?.command ?? payload?.body ?? '')}
                    </code>
                  </Row>
                  <Row value={payload?.host_name}>Machine</Row>
                  {payload?.harness_name && <Row value={payload.harness_name}>Harness</Row>}
                  {payload?.directory && <Row value={payload.directory}>Directory</Row>}
                </>
              ) : credential ? (
                <>
                  <Row value={payload?.domain}>Site</Row>
                  <Row value={payload?.username}>Username</Row>
                  <Row>
                    <span className="phone-row-copy">
                      <span>Pagis opens this address and types there.</span>
                      <span className="phone-hint">{payload?.login_url}</span>
                    </span>
                  </Row>
                </>
              ) : (
                <Row>
                  <pre className="approval-phone-body">
                    {Object.entries(args)
                      .map(
                        ([key, value]) =>
                          `${key}: ${typeof value === 'string' ? value : JSON.stringify(value)}`,
                      )
                      .join('\n') || payload?.body}
                  </pre>
                </Row>
              )}
            </Frame>
          </section>
          {pending && alwaysWords && (
            <>
              <Frame>
                <Switch
                  row
                  checked={always}
                  onCheckedChange={setAlways}
                  disabled={decide.isPending}
                >
                  <span className="phone-row-copy">
                    <span>{alwaysWords.lead}</span>
                    {alwaysWords.rules !== null && (
                      <code className="phone-hint">{alwaysWords.rules}</code>
                    )}
                  </span>
                </Switch>
              </Frame>
              {reach !== null && <p className="phone-hint">{reach}</p>}
            </>
          )}
          {!pending && (
            <Badge
              tone={
                row.state === 'approved' ? 'working' : row.state === 'denied' ? 'failed' : 'neutral'
              }
            >
              {label}
            </Badge>
          )}
          {decide.isError && (
            <p role="alert">
              {errorMessage(decide.error, 'The decision could not be saved. Try again.')}
            </p>
          )}
        </div>
      ) : (
        <p className="phone-hint">
          {request.isError ? 'This approval could not be read.' : 'Reading the approval…'}
        </p>
      )}
    </Sheet>
  )
}
