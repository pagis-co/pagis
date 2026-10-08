import { useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { ChevronRight, Phone } from 'lucide-react'
import type { AgentDto, ApiClient, MessageDto } from '../../api/client'
import { Avatar, Badge, Button, Frame, IconButton, Row, SectionLabel } from '../../primitives'
import { Blocks } from '../../blocks/BlockView'
import { useDecideRequest, useRequest } from '../../queries'
import { ChannelComposer } from '../composer/ChannelComposer'
import { LargeTitle } from '../phone/TopBar'
import { briefLine, homeDate, type RecordRow } from './report'
import { isDismissible, queueDetail, type QueueItem } from './queue'
import { runDuration, triggerText } from '../runs/runs'
import './home-phone.css'

export function age(at: number): string {
  const minutes = Math.max(0, Math.floor((Date.now() - at) / 60000))
  return minutes < 1
    ? 'Now'
    : minutes < 60
      ? `${minutes} min`
      : minutes < 1440
        ? `${Math.floor(minutes / 60)} h`
        : `${Math.floor(minutes / 1440)} d`
}

function ApprovalPreview({
  api,
  item,
  agent,
}: {
  api: ApiClient
  item: Extract<QueueItem, { kind: 'approval' }>
  agent?: AgentDto
}) {
  const decide = useDecideRequest(api, item.request_id)
  const request = useRequest(api, item.request_id)
  const navigate = useNavigate()
  return (
    <Frame data-testid="approval-card">
      <div className="home-phone-approval">
        <div className="home-phone-approval-head">
          <Avatar
            id={item.agent_id}
            name={agent?.name ?? 'Sprite'}
            appearance={agent?.avatar}
            size="md"
            presence="waiting"
          />
          <strong>{item.line}</strong>
          <span className="phone-row-time">{age(item.at)}</span>
        </div>
        <div className="phone-well">
          <strong>{item.title}</strong>
          <p className="phone-row-line">{item.body}</p>
          <Button
            variant="link"
            onClick={() =>
              void navigate({
                to: '.',
                search: (previous) => ({ ...previous, request: item.request_id }),
              })
            }
          >
            See the full{' '}
            {String((request.data?.payload as { tool_name?: string })?.tool_name ?? '').includes(
              'mail',
            )
              ? 'email'
              : 'action'}
            <ChevronRight size={16} aria-hidden />
          </Button>
        </div>
        <div className="phone-actions">
          <Button
            disabled={decide.isPending}
            onClick={() => decide.mutate({ decision: 'denied', scope: 'once' })}
          >
            Deny
          </Button>
          <Button
            variant="primary"
            disabled={decide.isPending}
            onClick={() => decide.mutate({ decision: 'approved', scope: 'once' })}
          >
            Approve
          </Button>
        </div>
        {decide.isError && <p role="alert">The decision could not be saved. Try again.</p>}
      </div>
    </Frame>
  )
}

function QueueRow({
  item,
  onOpen,
  onCallBack,
  onDismiss,
}: {
  item: QueueItem
  onOpen: () => void
  onCallBack: () => void
  onDismiss: () => void
}) {
  const [dismiss, setDismiss] = useState(false)
  const [start, setStart] = useState<number | null>(null)
  const detail =
    item.kind === 'approval'
      ? item.title
      : item.kind === 'call'
        ? `${item.remote_e164} · ${queueDetail(item)}`
        : queueDetail(item)
  return (
    <div
      className="home-phone-swipe"
      onPointerDown={(event) => setStart(event.clientX)}
      onPointerUp={(event) => {
        if (start !== null && start - event.clientX > 60 && isDismissible(item)) setDismiss(true)
        setStart(null)
      }}
    >
      <Row>
        <span
          className={`home-phone-dot${item.kind === 'failed' ? ' home-phone-dot-failed' : ''}`}
          aria-hidden
        />
        <Button variant="link" className="home-phone-queue-link" onClick={onOpen}>
          <span className="phone-row-copy">
            <strong>{item.line}</strong>
            {detail && <span className="phone-row-line">{detail}</span>}
          </span>
          <span className="phone-row-time">{item.kind === 'keypad' ? '' : age(item.at)}</span>
          {item.kind !== 'call' && <ChevronRight size={16} aria-hidden />}
        </Button>
        {item.kind === 'call' && (
          <IconButton icon={Phone} label="Call back" variant="link" onClick={onCallBack} />
        )}
        {dismiss && (
          <Button variant="danger" onClick={onDismiss}>
            Dismiss
          </Button>
        )}
      </Row>
    </div>
  )
}

export function HomePhone({
  api,
  agents,
  chief,
  channelId,
  items,
  count,
  report,
  writing,
  record,
  loading,
  queueError,
  recordError,
  onRetryQueue,
  onRetryRecord,
  onOpen,
  onDismiss,
  onOpenRun,
  onWrite,
  onReply,
}: {
  api: ApiClient
  agents: AgentDto[]
  chief: AgentDto | null
  channelId: string | null
  items: QueueItem[]
  count: number
  report: MessageDto | null
  writing: boolean
  record: RecordRow[]
  loading: boolean
  queueError: boolean
  recordError: boolean
  onRetryQueue: () => void
  onRetryRecord: () => void
  onOpen: (item: QueueItem) => void
  onDismiss: (item: QueueItem) => void
  onOpenRun: (id: string) => void
  onWrite?: () => void
  onReply: () => void
}) {
  const navigate = useNavigate()
  const newest = items.filter((item) => item.kind === 'approval').sort((a, b) => b.at - a.at)[0]
  return (
    <div className="home-phone" data-testid="home">
      <div className="home-phone-scroll">
        <LargeTitle
          title={homeDate(Date.now())}
          hint={briefLine(chief?.name ?? null, report?.created_at ?? null, writing)}
        />
        <div className="home-phone-sections">
          <section className="phone-section" aria-label="Needs you">
            <div className="home-phone-section-head">
              <SectionLabel>Needs you</SectionLabel>
              <Badge tone="waiting">{count}</Badge>
            </div>
            {queueError ? (
              <p role="alert" className="phone-hint">
                Could not load your decisions.{' '}
                <Button variant="link" onClick={onRetryQueue}>
                  Try again
                </Button>
              </p>
            ) : loading ? (
              <p className="phone-hint">Loading decisions…</p>
            ) : !items.length ? (
              <p className="phone-hint">Nothing needs you.</p>
            ) : (
              <>
                {newest && (
                  <ApprovalPreview
                    api={api}
                    item={newest}
                    agent={agents.find((agent) => agent.id === newest.agent_id)}
                  />
                )}
                {items.some((item) => item.id !== newest?.id) && (
                  <Frame>
                    {items
                      .filter((item) => item.id !== newest?.id)
                      .map((item) => (
                        <QueueRow
                          key={item.id}
                          item={item}
                          onDismiss={() => onDismiss(item)}
                          onCallBack={() => onOpen(item)}
                          onOpen={() => {
                            if (item.kind === 'approval')
                              void navigate({
                                to: '.',
                                search: (previous) => ({ ...previous, request: item.request_id }),
                              })
                            else if (item.kind === 'call')
                              void navigate({
                                to: '/calls/$callId',
                                params: { callId: item.call_id },
                              })
                            else onOpen(item)
                          }}
                        />
                      ))}
                  </Frame>
                )}
              </>
            )}
          </section>
          <section className="phone-section" aria-label="Your brief">
            <div className="phone-section-head">
              <SectionLabel>{chief ? `${chief.name}'s brief` : 'Your brief'}</SectionLabel>
              {onWrite && (
                <Button variant="link" disabled={writing} onClick={onWrite}>
                  Write a report now
                </Button>
              )}
            </div>
            <Frame>
              <div className="home-phone-brief">
                {report ? (
                  <Blocks api={api} blocks={report.blocks} />
                ) : (
                  <p className="phone-hint">No brief yet.</p>
                )}
                {chief && (
                  <Button variant="link" onClick={onReply}>
                    Reply to {chief.name}
                  </Button>
                )}
              </div>
            </Frame>
          </section>
          <section className="phone-section" aria-label="Work record">
            <div className="home-phone-section-head">
              <SectionLabel>Work record</SectionLabel>
              <span className="phone-hint">Today and before</span>
            </div>
            <Frame>
              {recordError ? (
                <Row>
                  <span role="alert">Could not read the work record.</span>
                  <Button variant="link" onClick={onRetryRecord}>
                    Try again
                  </Button>
                </Row>
              ) : record.length ? (
                record.map(({ run, stamp }) => (
                  <Row key={run.id} onClick={() => onOpenRun(run.id)}>
                    <Avatar
                      id={run.agent_id}
                      name={agents.find((agent) => agent.id === run.agent_id)?.name ?? 'Sprite'}
                      size="md"
                    />
                    <span className="phone-row-copy">
                      <strong>{run.title}</strong>
                      <span className="phone-row-line">
                        {agents.find((agent) => agent.id === run.agent_id)?.name} ·{' '}
                        {triggerText(run, null)} · {runDuration(run)}
                      </span>
                    </span>
                    <span className="phone-row-time">{stamp}</span>
                  </Row>
                ))
              ) : (
                <Row>No work has finished yet.</Row>
              )}
            </Frame>
          </section>
        </div>
      </div>
      {chief && channelId && (
        <div className="home-phone-composer">
          <ChannelComposer
            api={api}
            channelId={channelId}
            placeholder={`Message ${chief.name}`}
            adoptsDraft={false}
          />
        </div>
      )}
    </div>
  )
}
