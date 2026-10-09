// The Automations destination (ADR-0022): the workspace view of
// every durable rule. A needs-you queue sits at the top, then
// Schedules, then Event Subscriptions. Selecting a row opens the
// detail body, which carries the rule's current revision, state,
// creator, Agent, destination, timing or filter, next due, last
// occurrence, last Run result, and the paginated occurrence and
// Wake-up history from ADR-0006.
//
// A rule an Agent proposes is a pending Request until the user decides
// it, so it appears in the needs-you queue and not in the lists below:
// the broker approves at the tool call, before any row exists.

import { useMemo, useState, type ReactNode } from 'react'
import { useIsMobile } from '../state/useIsMobile'
import { NavBar } from './phone/TopBar'

import { X } from 'lucide-react'
import { useNavigate } from '@tanstack/react-router'

import type { ApiClient } from '../api/client'
import { ApprovalCard } from '../blocks/ApprovalCard'
import { ActionSheet, Avatar, Badge, Button, Frame, IconButton, Input, Row, SectionLabel, Textarea } from '../primitives'
import {
  errorMessage,
  useAgents,
  useChannels,
  useConnections,
  usePendingRequests,
  useSchedule,
  useScheduleOccurrences,
  useScheduleWakeups,
  useSchedules,
  useSubscription,
  useSubscriptionEvents,
  useSubscriptionWakeups,
  useSubscriptions,
  useUpdateSchedule,
  useUpdateSubscription,
} from '../queries'

import { threadScope } from '../timeline'
import { useComposerDraft } from '../state/composerDraft'
import { AskAnAgent } from './AskAnAgent'
import { nearMoment } from './when'

import './Automations.css'

/** What each section is, in the words the user would use. Every
 *  list on this page says what it holds before it lists anything. */
const EXPLANATIONS = {
  queue: 'Rules and connections that wait for your decision.',
  schedules: 'Work a sprite repeats on a clock, such as every weekday morning.',
  subscriptions:
    'Work a sprite starts when something happens, such as new mail from your accountant.',
  occurrences: 'Every time this schedule was due, and what came of it.',
  events: 'Every event that reached this subscription.',
  wakeups: 'Every time a sprite woke up to do this work.',
} as const

/** The tools that mint a rule. A pending Request for one of these is a
 *  rule waiting for the user, and the queue says so in those words. */
const RULE_TOOLS: Record<string, string> = {
  schedule_create: 'Schedule',
  schedule_update: 'Schedule',
  event_subscription_create: 'Event Subscription',
  event_subscription_update: 'Event Subscription',
}

export type Selection =
  | { kind: 'schedule'; id: string }
  | { kind: 'subscription'; id: string }

/** The event that wakes a subscription, in the words of the person.
 *  A kind this build does not name reads as an event. */
const EVENT_KIND_TEXT: Record<string, string> = {
  'mail.message_received': 'New mail',
  'call.ended': 'A call that ends',
  'coding_session.turn_ended': 'A coding session turn that ends',
  'coding_session.needs_decision': 'A coding session that needs a decision',
  'coding_session.ended': 'A coding session that ends',
}

function eventKindText(kind: string): string {
  return EVENT_KIND_TEXT[kind] ?? 'An event'
}

/** A rule state as a badge says it: `paused` reads `Paused`. */
function stateText(state: string): string {
  const words = state.replaceAll('_', ' ')
  return words.charAt(0).toUpperCase() + words.slice(1)
}

function when(unixMs: number | null | undefined): string {
  if (unixMs === null || unixMs === undefined) return '—'
  return new Date(unixMs).toLocaleString()
}

/** The rule's cadence in the words the user set it with. */
function cadence(schedule: {
  kind: string
  cron_expression?: string | null
  interval_ms?: number | null
  scheduled_at: number
  timezone: string
}): string {
  switch (schedule.kind) {
    case 'cron':
      return `${schedule.cron_expression ?? ''} in ${schedule.timezone}`
    case 'interval': {
      const minutes = Math.round((schedule.interval_ms ?? 0) / 60_000)
      return `Every ${minutes} minute${minutes === 1 ? '' : 's'}`
    }
    default:
      return `Once at ${when(schedule.scheduled_at)}`
  }
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </>
  )
}

/** One history list with the daemon's cursor behind "Show more". */
function History<T>({
  title,
  description,
  empty,
  items,
  hasMore,
  loading,
  onMore,
  row,
}: {
  title: string
  description: string
  empty: string
  items: T[]
  hasMore: boolean
  loading: boolean
  onMore: () => void
  row: (item: T) => { id: string; text: string }
}) {
  return (
    <section className="automations-history">
      <h4>{title}</h4>
      <p className="automations-explanation">{description}</p>
      {items.length === 0 && <p>{empty}</p>}
      {items.map((item) => {
        const line = row(item)
        return <p key={line.id}>{line.text}</p>
      })}
      {hasMore && (
        <Button size="sm" disabled={loading} onClick={onMore}>
          Show more
        </Button>
      )}
    </section>
  )
}

/** The needs-you queue (ADR-0022): rules that wait for a decision,
 *  Connections that need signing in again, and collectors the daemon
 *  blocked. Each item also reads on its own record; this is a view. */
function NeedsYou({
  api,
  onOpenSubscription,
  ask,
}: {
  api: ApiClient
  onOpenSubscription: (subscriptionId: string) => void
  ask: ReactNode
}) {
  const phone = useIsMobile()
  const requests = usePendingRequests(api)
  const connections = useConnections(api)
  const subscriptions = useSubscriptions(api)

  const ruleRequests = (requests.data ?? []).filter((request) => {
    const payload = request.payload as { tool_name?: string }
    return (
      request.kind === 'tool_action' &&
      payload.tool_name !== undefined &&
      payload.tool_name in RULE_TOOLS
    )
  })
  const reauth = (connections.data ?? []).filter(
    (connection) => connection.status === 'reauth_required',
  )
  const blocked = (subscriptions.data ?? []).filter(
    (subscription) => subscription.blocked_reason != null,
  )
  const total = ruleRequests.length + reauth.length + blocked.length

  if (phone && total === 0) return <p className="phone-hint">Nothing waits for you.</p>
  if (phone)
    return (
      <section className="phone-section" aria-label="Needs you">
        <SectionLabel>Needs you</SectionLabel>
        <p className="phone-hint">{EXPLANATIONS.queue}</p>
        <Frame>
          {ruleRequests.map((request) => (
            <PhoneRequestRow key={request.id} api={api} request={request} />
          ))}
          {reauth.map((connection) => (
            <Row key={connection.id} href="/settings/connections" chevron hint="Connect again">
              {connection.display_name}
            </Row>
          ))}
          {blocked.map((subscription) => (
            <Row
              key={subscription.id}
              chevron
              onClick={() => onOpenSubscription(subscription.id)}
              hint={subscription.blocked_reason}
            >
              {subscription.name}
            </Row>
          ))}
        </Frame>
      </section>
    )

  return (
    <section className="automations-queue" aria-label="Needs you">
      <h3>Needs you</h3>
      <p className="automations-explanation">{EXPLANATIONS.queue}</p>
      {total === 0 && (
        <>
          <p>Nothing waits for you.</p>
          {ask}
        </>
      )}
      {ruleRequests.map((request) => {
        const payload = request.payload as {
          tool_name: string
          action_title?: string
          body?: string
        }
        return (
          <div key={request.id} className="automations-queue-item">
            <span className="automations-queue-kind">
              {RULE_TOOLS[payload.tool_name]} waits for you
            </span>
            <ApprovalCard
              api={api}
              requestId={request.id}
              title={payload.action_title ?? payload.tool_name}
              body={payload.body ?? ''}
            />
          </div>
        )
      })}
      {reauth.map((connection) => (
        <div key={connection.id} className="automations-queue-item">
          <span className="automations-queue-kind">Connection needs you</span>
          <p>
            {connection.display_name} must be connected again. Settings →
            Connections holds the repair.
          </p>
        </div>
      ))}
      {blocked.map((subscription) => (
        <Button
          key={subscription.id}
          size="lg"
          className="automations-queue-item"
          onClick={() => onOpenSubscription(subscription.id)}
        >
          <span className="automations-queue-kind">Collector is blocked</span>
          <span>
            {subscription.name}: {subscription.blocked_reason}
          </span>
        </Button>
      ))}
    </section>
  )
}

function PhoneRequestRow({ api, request }: { api: ApiClient; request: { id: string; agent_id: string; payload: unknown } }) {
  const navigate = useNavigate()
  const agents = useAgents(api)
  const agent = agents.data?.find((agent) => agent.id === request.agent_id)
  const payload = request.payload as { action_title?: string; tool_name?: string }
  return <Row chevron onClick={() => void navigate({ to: '.', search: (previous) => ({ ...previous, request: request.id }) })}><Avatar id={request.agent_id} name={agent?.name ?? 'Sprite'} appearance={agent?.avatar} size="sm" presence="waiting" /><span className="phone-row-copy"><span>{payload.action_title ?? payload.tool_name}</span><span className="phone-hint">{agent?.name ?? 'Sprite'} needs your approval</span></span></Row>
}

/** The Schedule controls the daemon already serves. Skip-next
 *  carries the due instant it saw, so a Schedule that became due first
 *  answers 409 and the surface says which side won. */
function ScheduleControls({
  api,
  schedule,
}: {
  api: ApiClient
  schedule: { id: string; state: string; next_due_at?: number | null }
}) {
  const update = useUpdateSchedule(api)
  const phone = useIsMobile()
  const [archive, setArchive] = useState(false)
  const [failure, setFailure] = useState<string | null>(null)

  const run = (
    action: 'pause' | 'resume' | 'skip_next' | 'archive',
    fallback: string,
  ) => {
    setFailure(null)
    update.mutate(
      {
        scheduleId: schedule.id,
        action,
        ...(action === 'skip_next' && schedule.next_due_at != null
          ? { expected_due_at: schedule.next_due_at }
          : {}),
      },
      { onError: (error) => setFailure(errorMessage(error, fallback)) },
    )
  }

  const live = schedule.state === 'active'
  return (
    <div className="automations-controls">
      {live ? (
        <Button disabled={update.isPending} onClick={() => run('pause', 'Pause failed.')}>
          Pause
        </Button>
      ) : (
        <Button
          disabled={update.isPending || schedule.state === 'archived'}
          onClick={() => run('resume', 'Resume failed.')}
        >
          Resume
        </Button>
      )}
      <Button
        disabled={update.isPending || schedule.next_due_at == null}
        onClick={() => run('skip_next', 'Skip next failed.')}
      >
        Skip next
      </Button>
      <Button
        variant="danger"
        disabled={update.isPending || schedule.state === 'archived'}
        onClick={() => phone ? setArchive(true) : run('archive', 'Archive failed.')}
      >
        Archive
      </Button>
      {failure !== null && <p className="automations-error">{failure}</p>}
      {phone && <ActionSheet open={archive} onOpenChange={setArchive} title="Archive this rule?" description="It stops starting new work. Its past work stays in the record." action={{ label: 'Archive', danger: true, disabled: update.isPending, onSelect: () => run('archive', 'Archive failed.') }} />}
    </div>
  )
}

/** Edit is one revision (ADR-0006): the daemon writes a new revision
 *  and reschedules from it. */
function ScheduleEdit({
  api,
  schedule,
}: {
  api: ApiClient
  schedule: {
    id: string
    name: string
    instruction: string
    revision: number
    kind: string
    cron_expression?: string | null
    interval_ms?: number | null
  }
}) {
  const update = useUpdateSchedule(api)
  const [open, setOpen] = useState(false)
  const [name, setName] = useState(schedule.name)
  const [instruction, setInstruction] = useState(schedule.instruction)
  const [failure, setFailure] = useState<string | null>(null)

  if (!open) {
    return (
      <Button className="automations-edit-open" onClick={() => setOpen(true)}>
        Edit
      </Button>
    )
  }
  return (
    <form
      className="automations-edit"
      onSubmit={(event) => {
        event.preventDefault()
        setFailure(null)
        update.mutate(
          {
            scheduleId: schedule.id,
            action: 'edit',
            expected_revision: schedule.revision,
            name,
            instruction,
          },
          {
            onSuccess: () => setOpen(false),
            onError: (error) => setFailure(errorMessage(error, 'Edit failed.')),
          },
        )
      }}
    >
      <label>
        Name
        <Input value={name} onChange={(event) => setName(event.target.value)} />
      </label>
      <label>
        Instruction
        <Textarea
          value={instruction}
          onChange={(event) => setInstruction(event.target.value)}
        />
      </label>
      <div className="automations-edit-actions">
        <Button type="submit" variant="primary" disabled={update.isPending}>
          Save
        </Button>
        <Button onClick={() => setOpen(false)}>Cancel</Button>
      </div>
      {failure !== null && <p className="automations-error">{failure}</p>}
    </form>
  )
}

function ScheduleDetail({
  api,
  scheduleId,
  names,
  onBack,
}: {
  api: ApiClient
  scheduleId: string
  names: { agents: Map<string, string>; channels: Map<string, string> }
  onBack: () => void
}) {
  const schedule = useSchedule(api, scheduleId)
  const occurrences = useScheduleOccurrences(api, scheduleId)
  const wakeups = useScheduleWakeups(api, scheduleId)

  const occurrenceItems = (occurrences.data?.pages ?? []).flatMap(
    (page) => page.items,
  )
  const wakeupItems = (wakeups.data?.pages ?? []).flatMap((page) => page.items)
  const row = schedule.data

  return (
    <section className="automations-detail">
      <Button className="automations-back" onClick={onBack}>
        Back to automations
      </Button>
      {row === undefined ? (
        <p>Loading…</p>
      ) : (
        <>
          <h3>{row.name}</h3>
          <dl className="automations-fields">
            <Field label="State">{row.state.replaceAll('_', ' ')}</Field>
            <Field label="Revision">{row.revision}</Field>
            <Field label="Creator">{row.creator}</Field>
            <Field label="Sprite">{names.agents.get(row.agent_id) ?? row.agent_id}</Field>
            <Field label="Destination">
              {names.channels.get(row.channel_id) ?? row.channel_id}
            </Field>
            <Field label="Timing">{cadence(row)}</Field>
            <Field label="Next due">{when(row.next_due_at)}</Field>
            <Field label="Last occurrence">
              {occurrenceItems.length === 0
                ? '—'
                : `${when(occurrenceItems[0].processed_at)} (${occurrenceItems[0].outcome})`}
            </Field>
            <Field label="Last Run result">{row.last_result ?? '—'}</Field>
            <Field label="Instruction">{row.instruction}</Field>
          </dl>
          <ScheduleControls api={api} schedule={row} />
          <ScheduleEdit api={api} schedule={row} />
          <History
            title="Occurrences"
            description={EXPLANATIONS.occurrences}
            empty="No occurrence yet."
            items={occurrenceItems}
            hasMore={occurrences.hasNextPage}
            loading={occurrences.isFetchingNextPage}
            onMore={() => void occurrences.fetchNextPage()}
            row={(item) => ({
              id: item.id,
              text: `${when(item.scheduled_at)} — ${item.outcome} (revision ${item.schedule_revision})`,
            })}
          />
          <History
            title="Wake-ups"
            description={EXPLANATIONS.wakeups}
            empty="No Wake-up yet."
            items={wakeupItems}
            hasMore={wakeups.hasNextPage}
            loading={wakeups.isFetchingNextPage}
            onMore={() => void wakeups.fetchNextPage()}
            row={(item) => ({
              id: item.id,
              text: `${when(item.scheduled_at)} — ${item.state} (${item.source_count} source${item.source_count === 1 ? '' : 's'})`,
            })}
          />
        </>
      )}
    </section>
  )
}

function SubscriptionControls({
  api,
  subscription,
}: {
  api: ApiClient
  subscription: { id: string; state: string }
}) {
  const update = useUpdateSubscription(api)
  const phone = useIsMobile()
  const [archive, setArchive] = useState(false)
  const [failure, setFailure] = useState<string | null>(null)

  const run = (action: 'pause' | 'resume' | 'archive', fallback: string) => {
    setFailure(null)
    update.mutate(
      { subscriptionId: subscription.id, action },
      { onError: (error) => setFailure(errorMessage(error, fallback)) },
    )
  }

  return (
    <div className="automations-controls">
      {subscription.state === 'active' ? (
        <Button disabled={update.isPending} onClick={() => run('pause', 'Pause failed.')}>
          Pause
        </Button>
      ) : (
        <Button
          disabled={update.isPending || subscription.state === 'archived'}
          onClick={() => run('resume', 'Resume failed.')}
        >
          Resume
        </Button>
      )}
      <Button
        variant="danger"
        disabled={update.isPending || subscription.state === 'archived'}
        onClick={() => phone ? setArchive(true) : run('archive', 'Archive failed.')}
      >
        Archive
      </Button>
      {failure !== null && <p className="automations-error">{failure}</p>}
      {phone && <ActionSheet open={archive} onOpenChange={setArchive} title="Archive this rule?" description="It stops starting new work. Its past work stays in the record." action={{ label: 'Archive', danger: true, disabled: update.isPending, onSelect: () => run('archive', 'Archive failed.') }} />}
    </div>
  )
}

function SubscriptionEdit({
  api,
  subscription,
}: {
  api: ApiClient
  subscription: { id: string; name: string; instruction: string; filter: unknown }
}) {
  const update = useUpdateSubscription(api)
  const [open, setOpen] = useState(false)
  const [name, setName] = useState(subscription.name)
  const [instruction, setInstruction] = useState(subscription.instruction)
  const [filter, setFilter] = useState(() =>
    JSON.stringify(subscription.filter ?? {}, null, 2),
  )
  const [failure, setFailure] = useState<string | null>(null)

  if (!open) {
    return (
      <Button className="automations-edit-open" onClick={() => setOpen(true)}>
        Edit
      </Button>
    )
  }
  return (
    <form
      className="automations-edit"
      onSubmit={(event) => {
        event.preventDefault()
        setFailure(null)
        let parsed: unknown
        try {
          parsed = JSON.parse(filter)
        } catch {
          setFailure('The filter is not valid JSON.')
          return
        }
        update.mutate(
          {
            subscriptionId: subscription.id,
            action: 'edit',
            name,
            instruction,
            filter: parsed,
          },
          {
            onSuccess: () => setOpen(false),
            onError: (error) => setFailure(errorMessage(error, 'Edit failed.')),
          },
        )
      }}
    >
      <label>
        Name
        <Input value={name} onChange={(event) => setName(event.target.value)} />
      </label>
      <label>
        Instruction
        <Textarea
          value={instruction}
          onChange={(event) => setInstruction(event.target.value)}
        />
      </label>
      <label>
        Filter
        <Textarea value={filter} onChange={(event) => setFilter(event.target.value)} />
      </label>
      <div className="automations-edit-actions">
        <Button type="submit" variant="primary" disabled={update.isPending}>
          Save
        </Button>
        <Button onClick={() => setOpen(false)}>Cancel</Button>
      </div>
      {failure !== null && <p className="automations-error">{failure}</p>}
    </form>
  )
}

function SubscriptionDetail({
  api,
  subscriptionId,
  names,
  onBack,
}: {
  api: ApiClient
  subscriptionId: string
  names: { agents: Map<string, string>; channels: Map<string, string> }
  onBack: () => void
}) {
  const subscription = useSubscription(api, subscriptionId)
  const events = useSubscriptionEvents(api, subscriptionId)
  const wakeups = useSubscriptionWakeups(api, subscriptionId)

  const eventItems = (events.data?.pages ?? []).flatMap((page) => page.items)
  const wakeupItems = (wakeups.data?.pages ?? []).flatMap((page) => page.items)
  const row = subscription.data

  return (
    <section className="automations-detail">
      <Button className="automations-back" onClick={onBack}>
        Back to automations
      </Button>
      {row === undefined ? (
        <p>Loading…</p>
      ) : (
        <>
          <h3>{row.name}</h3>
          <dl className="automations-fields">
            <Field label="State">{row.state.replaceAll('_', ' ')}</Field>
            <Field label="Revision">{row.revision}</Field>
            <Field label="Creator">{row.creator}</Field>
            <Field label="Sprite">{names.agents.get(row.agent_id) ?? row.agent_id}</Field>
            <Field label="Destination">
              {names.channels.get(row.channel_id) ?? row.channel_id}
            </Field>
            <Field label="Source">{row.event_kind}</Field>
            <Field label="Filter">
              <code>{JSON.stringify(row.filter ?? {})}</code>
            </Field>
            <Field label="Last collection">
              {row.last_collection == null
                ? '—'
                : `${when(row.last_collection.collected_at)} — ${row.last_collection.outcome}`}
            </Field>
            <Field label="Last matched event">
              {row.last_matched_event == null
                ? '—'
                : when(row.last_matched_event.occurred_at)}
            </Field>
            <Field label="Last Run result">
              {wakeupItems.length === 0 ? '—' : wakeupItems[0].state}
            </Field>
            {row.blocked_reason != null && (
              <Field label="Blocked">{row.blocked_reason}</Field>
            )}
            <Field label="Instruction">{row.instruction}</Field>
          </dl>
          <SubscriptionControls api={api} subscription={row} />
          <SubscriptionEdit api={api} subscription={row} />
          <History
            title="Incoming events"
            description={EXPLANATIONS.events}
            empty="No event yet."
            items={eventItems}
            hasMore={events.hasNextPage}
            loading={events.isFetchingNextPage}
            onMore={() => void events.fetchNextPage()}
            row={(item) => ({
              id: item.id,
              text: `${when(item.occurred_at)} — ${item.event_kind}`,
            })}
          />
          <History
            title="Wake-ups"
            description={EXPLANATIONS.wakeups}
            empty="No Wake-up yet."
            items={wakeupItems}
            hasMore={wakeups.hasNextPage}
            loading={wakeups.isFetchingNextPage}
            onMore={() => void wakeups.fetchNextPage()}
            row={(item) => ({
              id: item.id,
              text: `${when(item.scheduled_at)} — ${item.state} (${item.source_count} source${item.source_count === 1 ? '' : 's'})`,
            })}
          />
        </>
      )}
    </section>
  )
}

export function Automations({
  api,
  onClose,
  onOpenChannel,
  selection,
  onSelect,
}: {
  api: ApiClient
  onClose: () => void
  /** Open one channel: an empty section sends the reader to a DM. */
  onOpenChannel: (channelId: string) => void
  selection?: Selection | null
  onSelect?: (selection: Selection | null) => void
}) {
  const phone = useIsMobile()
  const [localSelected, setLocalSelected] = useState<Selection | null>(null)
  const selected = phone && selection !== undefined ? selection : localSelected
  const setSelected = (next: Selection | null) => phone && onSelect ? onSelect(next) : setLocalSelected(next)
  const schedules = useSchedules(api)
  const subscriptions = useSubscriptions(api)
  const agents = useAgents(api)
  const channels = useChannels(api)
  const setDraft = useComposerDraft((state) => state.set)

  const ask = (kind: 'automation' | 'schedule' | 'subscription') => (
    <AskAnAgent
      kind={kind}
      agents={agents.data ?? []}
      channels={channels.data ?? []}
      onOpenChannel={onOpenChannel}
      onDraft={(channelId, text) => setDraft(threadScope(channelId), text)}
    />
  )

  const names = useMemo(
    () => ({
      agents: new Map((agents.data ?? []).map((agent) => [agent.id, agent.name])),
      channels: new Map(
        (channels.data ?? []).map((channel) => [
          channel.id,
          channel.title ?? 'Untitled',
        ]),
      ),
    }),
    [agents.data, channels.data],
  )

  if (phone) {
    const avatarOf = (agentId: string) => (
      <Avatar
        id={agentId}
        name={names.agents.get(agentId) ?? 'Sprite'}
        appearance={agents.data?.find((agent) => agent.id === agentId)?.avatar}
        size="md"
      />
    )
    return (
      <>
        <NavBar
          back={{
            label: selected ? 'Automations' : 'You',
            onBack: selected ? () => setSelected(null) : onClose,
          }}
        />
        <div className="phone-content phone-automations">
          {selected ? (
            selected.kind === 'schedule' ? (
              <ScheduleDetail
                api={api}
                scheduleId={selected.id}
                names={names}
                onBack={() => setSelected(null)}
              />
            ) : (
              <SubscriptionDetail
                api={api}
                subscriptionId={selected.id}
                names={names}
                onBack={() => setSelected(null)}
              />
            )
          ) : (
            <>
              <h1 className="phone-heading">Automations</h1>
              <NeedsYou
                api={api}
                onOpenSubscription={(id) => setSelected({ kind: 'subscription', id })}
                ask={ask('automation')}
              />
              <section className="phone-section">
                <SectionLabel>Schedules</SectionLabel>
                <p className="phone-hint">{EXPLANATIONS.schedules}</p>
                <Frame>
                  {(schedules.data ?? []).map((row) => (
                    <Row
                      key={row.id}
                      chevron
                      onClick={() => setSelected({ kind: 'schedule', id: row.id })}
                    >
                      {avatarOf(row.agent_id)}
                      <span className="phone-row-copy">
                        <span>{row.name}</span>
                        <span className="phone-hint">
                          {[
                            row.state === 'active' && row.next_due_at != null
                              ? `Next: ${nearMoment(row.next_due_at)}`
                              : null,
                            names.agents.get(row.agent_id),
                          ]
                            .filter(Boolean)
                            .join(' · ')}
                        </span>
                      </span>
                      {row.state !== 'active' && <Badge>{stateText(row.state)}</Badge>}
                    </Row>
                  ))}
                </Frame>
                {schedules.data?.length === 0 && ask('schedule')}
              </section>
              <section className="phone-section">
                <SectionLabel>Event Subscriptions</SectionLabel>
                <p className="phone-hint">{EXPLANATIONS.subscriptions}</p>
                <Frame>
                  {(subscriptions.data ?? []).map((row) => (
                    <Row
                      key={row.id}
                      chevron
                      onClick={() => setSelected({ kind: 'subscription', id: row.id })}
                    >
                      {avatarOf(row.agent_id)}
                      <span className="phone-row-copy">
                        <span>{row.name}</span>
                        <span className="phone-hint">
                          {[eventKindText(row.event_kind), names.agents.get(row.agent_id)]
                            .filter(Boolean)
                            .join(' · ')}
                        </span>
                      </span>
                      {row.state !== 'active' && <Badge>{stateText(row.state)}</Badge>}
                    </Row>
                  ))}
                </Frame>
                {subscriptions.data?.length === 0 && ask('subscription')}
              </section>
              <p className="phone-hint">To add one, ask a sprite to set it up.</p>
              {(schedules.isError || subscriptions.isError) && (
                <p role="alert" className="phone-hint">
                  Could not read the automations.
                </p>
              )}
            </>
          )}
        </div>
      </>
    )
  }
  return (
    <div className="automations-panel">
      <header className="automations-header">
        <h2>Automations</h2>
        <IconButton
          icon={X}
          label="Close automations"
          variant="ghost"
          onClick={onClose}
        />
      </header>

      {selected === null ? (
        <>
          <NeedsYou
            api={api}
            onOpenSubscription={(id) => setSelected({ kind: 'subscription', id })}
            ask={ask('automation')}
          />
          <section className="automations-section" aria-label="Schedules">
            <h3>Schedules</h3>
            <p className="automations-explanation">{EXPLANATIONS.schedules}</p>
            {schedules.data?.length === 0 && (
              <>
                <p>No Schedule yet.</p>
                {ask('schedule')}
              </>
            )}
            {(schedules.data ?? []).map((schedule) => (
              <Button
                key={schedule.id}
                className="automations-row"
                onClick={() => setSelected({ kind: 'schedule', id: schedule.id })}
              >
                <Avatar
                  appearance={(agents.data ?? []).find((agent) => agent.id === schedule.agent_id)?.avatar} id={schedule.agent_id}
                  name={names.agents.get(schedule.agent_id) ?? schedule.agent_id}
                  size="sm"
                />
                <span>{schedule.name}</span>
                <span className="automations-row-state">
                  {schedule.state.replaceAll('_', ' ')}
                </span>
                <span>Next {when(schedule.next_due_at)}</span>
                <span>Last {schedule.last_result ?? '—'}</span>
              </Button>
            ))}
          </section>
          <section className="automations-section" aria-label="Event Subscriptions">
            <h3>Event Subscriptions</h3>
            <p className="automations-explanation">{EXPLANATIONS.subscriptions}</p>
            {subscriptions.data?.length === 0 && (
              <>
                <p>No Event Subscription yet.</p>
                {ask('subscription')}
              </>
            )}
            {(subscriptions.data ?? []).map((subscription) => (
              <Button
                key={subscription.id}
                className="automations-row"
                onClick={() =>
                  setSelected({ kind: 'subscription', id: subscription.id })
                }
              >
                <Avatar
                  appearance={(agents.data ?? []).find((agent) => agent.id === subscription.agent_id)?.avatar} id={subscription.agent_id}
                  name={
                    names.agents.get(subscription.agent_id) ?? subscription.agent_id
                  }
                  size="sm"
                />
                <span>{subscription.name}</span>
                <span className="automations-row-state">
                  {subscription.state.replaceAll('_', ' ')}
                </span>
                <span>{subscription.event_kind}</span>
                <span>{subscription.blocked_reason ?? ''}</span>
              </Button>
            ))}
          </section>
        </>
      ) : selected.kind === 'schedule' ? (
        <ScheduleDetail
          api={api}
          scheduleId={selected.id}
          names={names}
          onBack={() => setSelected(null)}
        />
      ) : (
        <SubscriptionDetail
          api={api}
          subscriptionId={selected.id}
          names={names}
          onBack={() => setSelected(null)}
        />
      )}
    </div>
  )
}
