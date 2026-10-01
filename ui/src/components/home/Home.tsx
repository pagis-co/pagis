// `/`: Home, the Report of the Chief of Staff (ADR-0022).
//
// Home reads top to bottom as one page of the day: the date, whose
// brief this is, what needs the user, the Report prose the Chief of
// Staff wrote, the work record under it, and the composer that speaks
// to the Chief of Staff. The Desks sit in the panel beside it.
//
// The queue is live without a reload, because the runs that wait come
// from the presence store, which the WS firehose folds frame by frame,
// and the approvals and the failures come from queries the same
// firehose invalidates.

import { CheckCheck, Clock3, Menu as MenuIcon, Monitor, PenLine } from 'lucide-react'
import { useMemo } from 'react'

import type { ApiClient } from '../../api/client'
import { ApprovalCard, decidedClock } from '../../blocks/ApprovalCard'
import { Blocks } from '../../blocks/BlockView'
import { Card, CardBody, CardFooter } from '../../blocks/Card'
import { Avatar, Badge, Button, IconButton } from '../../primitives'
import {
  useAgentNames,
  useAgents,
  useChannels,
  useClearKeypadFailures,
  useDismissCall,
  useDismissRun,
  useReport,
  useRunScheduleNow,
  useRuns,
  useWorkspace,
} from '../../queries'
import { useComposerDraft } from '../../state/composerDraft'
import { usePresence } from '../../state/presence'
import { threadScope } from '../../timeline'
import { directMessageChannel } from '../AskAnAgent'
import { PageState } from '../PageState'
import { ChannelComposer } from '../composer/ChannelComposer'
import { runDuration, triggerText } from '../runs/runs'
import { chiefOfStaff } from '../sidebar/conversations'
import { briefLine, homeDate, workRecord } from './report'
import { callBackDraft, isDismissible, queueAction, queueLine, type QueueItem } from './queue'
import { useQueue } from './useQueue'

import './home.css'

/** One row of the queue. The row is one card frame: a dot in the
 *  state hue, the line that says what waits, the detail under it, and
 *  one primary action in the footer, with Dismiss at its end when the
 *  item only tells the reader. A pending approval keeps its own card,
 *  which carries the decision. */
function QueueRow({
  api,
  item,
  agentName,
  onOpen,
  onDismiss = null,
  busy = false,
}: {
  api: ApiClient
  item: QueueItem
  agentName: string
  onOpen: () => void
  /** Takes the item out of the queue; `null` where only the action
   *  of the item settles it. */
  onDismiss?: (() => void) | null
  /** The action of the row runs now. */
  busy?: boolean
}) {
  const line = queueLine(item, agentName)
  if (item.kind === 'approval') {
    return (
      <li className="home-queue-item" data-kind={item.kind}>
        <p className="home-queue-asks">
          <span className="home-queue-dot" aria-hidden />
          {line}
        </p>
        <ApprovalCard
          api={api}
          requestId={item.requestId}
          title={item.title}
          body={item.body}
        />
      </li>
    )
  }
  // The keypad notice states the end of its delay in the detail, so
  // it carries no clock of its own.
  const clock = item.kind === 'keypad' ? null : decidedClock(item.at)
  return (
    <li className="home-queue-item" data-kind={item.kind}>
      <Card className="home-queue-card" data-kind={item.kind}>
        <CardBody className="home-queue-body">
          <span className="home-queue-head">
            <span className="home-queue-dot" aria-hidden />
            <span className="home-queue-line">{line}</span>
            {clock !== null && <span className="home-queue-clock">{clock}</span>}
          </span>
          {item.detail !== null && <p className="home-queue-detail">{item.detail}</p>}
        </CardBody>
        <CardFooter>
          <Button variant="primary" size="sm" disabled={busy} onClick={onOpen}>
            {queueAction(item)}
          </Button>
          {onDismiss !== null && (
            <Button variant="ghost" size="sm" onClick={onDismiss}>
              Dismiss
            </Button>
          )}
        </CardFooter>
      </Card>
    </li>
  )
}

export function Home({
  api,
  onOpenChannel,
  onOpenRun,
  onOpenNav,
  desk = null,
}: {
  api: ApiClient
  onOpenChannel: (channelId: string) => void
  onOpenRun: (runId: string) => void
  onOpenNav: () => void
  /** The Desk Panel toggle, where the person opens the panel; `null`
   *  where the route keeps it open. */
  desk?: { open: boolean; onToggle: () => void } | null
}) {
  const agents = useAgents(api)
  const agentNames = useAgentNames(api)
  const channels = useChannels(api)
  const workspace = useWorkspace(api)
  const report = useReport(api)
  const writeNow = useRunScheduleNow(api)
  const queue = useQueue(api)
  const clearKeypad = useClearKeypadFailures(api)
  const dismissRun = useDismissRun(api)
  const dismissCall = useDismissCall(api)
  // The record answers what the presence store does not hold: the work
  // that is done.
  const completed = useRuns(api, '', '', 'completed')
  const setDraft = useComposerDraft((state) => state.set)

  const chief = chiefOfStaff(agents.data ?? [], workspace.data?.chief_of_staff_agent_id)
  const chiefChannelId =
    chief === null ? null : directMessageChannel(channels.data ?? [], chief.id)
  const writingRunId = report.data?.writing_run_id ?? null
  // A Report that runs now is in the presence store too, so the line
  // stays right through the whole Run and not only at its first read.
  const chiefWorking = usePresence((state) =>
    Object.values(state.runs).some((run) => run.runId === writingRunId),
  )
  const writing = writingRunId !== null && chiefWorking

  const record = useMemo(() => workRecord(completed.data ?? []), [completed.data])

  const channelNames = useMemo(
    () =>
      new Map(
        (channels.data ?? []).map((channel) => [channel.id, channel.title ?? 'Untitled']),
      ),
    [channels.data],
  )

  /** Takes a missed call or a failure out of the queue. */
  const dismissItem = (item: QueueItem) => {
    if (item.kind === 'call') dismissCall.mutate(item.id)
    if (item.kind === 'failed') dismissRun.mutate(item.runId)
  }

  /** A failed run opens its run, a waiting run its conversation, and a
   *  missed call the Agent's DM with the call-back message already
   *  written: the call then goes through the Call Brief and the
   *  approval card like every other call (ADR-0020). The keypad notice
   *  clears the failed-attempt count of the Workspace (ADR-0021). A
   *  missed call or a failure the reader acts on leaves the queue. */
  const openItem = (item: QueueItem) => {
    if (item.kind === 'approval') return
    if (item.kind === 'keypad') return clearKeypad.mutate()
    if (item.kind === 'call') {
      const channelId = directMessageChannel(channels.data ?? [], item.agentId)
      if (channelId === null) return
      setDraft(threadScope(channelId), callBackDraft(item))
      dismissItem(item)
      return onOpenChannel(channelId)
    }
    if (item.kind === 'failed') {
      dismissItem(item)
      return onOpenRun(item.runId)
    }
    if (item.channelId !== null) return onOpenChannel(item.channelId)
    return onOpenRun(item.runId)
  }

  const queueReady = !queue.isPending && !queue.isError
  const reportMessage = report.data?.message ?? null
  const scheduleId = report.data?.schedule_id ?? null

  return (
    <div className="home" data-testid="home">
      <header className="home-header">
        <IconButton
          icon={MenuIcon}
          label="Open conversations"
          variant="ghost"
          className="mobile-navigation-trigger"
          onClick={onOpenNav}
        />
        <div className="home-header-title">
          <h2>{homeDate(Date.now())}</h2>
          <p className="page-intro">
            {briefLine(chief?.name ?? null, reportMessage?.created_at ?? null, writing)}
          </p>
        </div>
        {scheduleId !== null && (
          <Button
            size="sm"
            className="home-write-report"
            disabled={writeNow.isPending || writing}
            onClick={() => writeNow.mutate(scheduleId)}
          >
            <PenLine size={14} aria-hidden />
            Write a report now
          </Button>
        )}
        {desk !== null && (
          <IconButton
            icon={Monitor}
            label="Desk panel"
            variant="ghost"
            aria-pressed={desk.open}
            onClick={desk.onToggle}
          />
        )}
      </header>

      <section className="home-section" aria-label="Needs you">
        <div className="home-section-heading">
          <h3>Needs you</h3>
          {queueReady && <Badge tone="neutral">{queue.items.length}</Badge>}
        </div>
        {queue.isError ? (
          <PageState icon={CheckCheck} title="Could not load your decisions" onRetry={queue.refetch}>
            Your approvals and missed calls may be out of date.
          </PageState>
        ) : queue.isPending ? (
          <PageState icon={CheckCheck} title="Loading decisions…" />
        ) : queue.items.length === 0 ? (
          <PageState icon={CheckCheck} title="All caught up">Nothing needs you.</PageState>
        ) : (
          <ul className="home-queue">
            {queue.items.map((item) => (
              <QueueRow
                key={`${item.kind}-${item.id}`}
                api={api}
                item={item}
                agentName={
                  item.kind === 'keypad' ? '' : (agentNames[item.agentId] ?? 'A sprite')
                }
                onOpen={() => openItem(item)}
                onDismiss={isDismissible(item) ? () => dismissItem(item) : null}
                busy={item.kind === 'keypad' && clearKeypad.isPending}
              />
            ))}
          </ul>
        )}
      </section>

      <section className="home-section home-report" aria-label="Your brief">
        {chief !== null && reportMessage !== null ? (
          <article className="home-report-prose" data-testid="home-report">
            <Avatar
              id={chief.id}
              name={chief.name}
              appearance={chief.avatar}
              size="sm"
            />
            <div className="home-report-body">
              <Blocks api={api} blocks={reportMessage.blocks} />
              {writing && <Badge tone="working">Writing</Badge>}
            </div>
          </article>
        ) : report.isPending ? (
          <PageState icon={PenLine} title="Loading your brief…" />
        ) : (
          <PageState icon={PenLine} title="No brief yet">
            {chief === null
              ? 'Name a chief of staff on the Staff page, and the brief starts.'
              : `${chief.name} writes your brief every morning. Ask for one now with the button above.`}
          </PageState>
        )}
      </section>

      <section className="home-section" aria-label="Work record">
        <div className="home-section-heading">
          <h3>Work record</h3>
          <span className="home-section-note">Today and before</span>
        </div>
        {completed.isError ? (
          <PageState icon={Clock3} title="Could not load the work record" onRetry={() => { void completed.refetch() }} />
        ) : completed.isPending ? (
          <PageState icon={Clock3} title="Loading the work record…" />
        ) : record.length === 0 ? (
          <PageState icon={Clock3} title="A fresh start">No work has finished yet.</PageState>
        ) : (
          <ul className="home-recent">
            {record.map(({ run, stamp }) => (
              <li key={run.id}>
                <Button
                  variant="ghost"
                  size="lg"
                  className="home-recent-row"
                  onClick={() => onOpenRun(run.id)}
                >
                  <Avatar
                    appearance={(agents.data ?? []).find((agent) => agent.id === run.agent_id)?.avatar}
                    id={run.agent_id}
                    name={agentNames[run.agent_id] ?? 'A sprite'}
                    size="sm"
                  />
                  <span className="home-recent-what">
                    <strong>{agentNames[run.agent_id] ?? 'A sprite'}</strong>
                    <span className="home-recent-trigger">
                      {triggerText(
                        run,
                        run.channel_id == null ? null : channelNames.get(run.channel_id) ?? null,
                      )}
                    </span>
                  </span>
                  <span className="home-recent-duration">{runDuration(run)}</span>
                  <span className="home-recent-stamp">{stamp}</span>
                </Button>
              </li>
            ))}
          </ul>
        )}
      </section>

      {chiefChannelId !== null && chief !== null && (
        <div className="home-composer">
          <ChannelComposer
            api={api}
            channelId={chiefChannelId}
            placeholder={`Message ${chief.name}`}
            adoptsDraft={false}
          />
        </div>
      )}
    </div>
  )
}
