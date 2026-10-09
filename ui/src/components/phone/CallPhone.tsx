import { useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import {
  AudioLines,
  ChevronDown,
  Phone,
  PhoneMissed,
  PhoneOff,
  UserRoundMinus,
  Volume2,
} from 'lucide-react'
import type { ApiClient } from '../../api/client'
import { ActionSheet, Avatar, Badge, Button, Frame, Row, SectionLabel } from '../../primitives'
import {
  useAgents,
  useCall,
  useChannels,
  useDismissCall,
  useDropCallTier,
  useHangUpCall,
} from '../../queries'
import {
  callDuration,
  callStateChip,
  formatDuration,
  formatE164,
  mergeTranscript,
  outcomeText,
  tierLabel,
  tierMeaning,
} from '../../blocks/call'
import { useNow } from '../../blocks/CallBlock'
import { useListenLive } from '../../blocks/ListenLive'
import { Transcript } from '../../blocks/Transcript'
import { selectCallLines, useCallTranscripts } from '../../state/stores'
import { useComposerDraft } from '../../state/composerDraft'
import { usePresence } from '../../state/presence'
import { threadScope } from '../../timeline'
import { directMessageChannel } from '../AskAnAgent'
import { callBackDraft } from '../home/queue'
import { changeTimeLabel } from '../memory/pages'
import { NavBar } from './TopBar'

export function CallPhone({ api, callId }: { api: ApiClient; callId: string }) {
  const navigate = useNavigate()
  const call = useCall(api, callId)
  const agents = useAgents(api)
  const channels = useChannels(api)
  const dismiss = useDismissCall(api)
  const hangup = useHangUpCall(api, callId)
  const drop = useDropCallTier(api, callId)
  const listen = useListenLive(callId)
  const buffered = useCallTranscripts(selectCallLines(callId))
  const caption = usePresence(
    (state) =>
      Object.values(state.runs).find((run) => run.agentId === call.data?.agent_id)?.caption ?? null,
  )
  const [confirm, setConfirm] = useState<'hangup' | 'drop' | null>(null)
  const live = call.data !== undefined && call.data.state !== 'ended'
  const now = useNow(live)
  const home = () => void navigate({ to: '/' })
  const record = call.data
  if (!record)
    return (
      <>
        <NavBar back={{ label: 'Home', onBack: home }} />
        <p className="phone-content" role={call.isError ? 'alert' : 'status'}>
          {call.isError ? 'That call is not on record.' : 'Reading the call…'}
        </p>
      </>
    )
  const agent = agents.data?.find((row) => row.id === record.agent_id)
  const lines = mergeTranscript(record.transcript, buffered)
  const channelId = directMessageChannel(channels.data ?? [], record.agent_id)
  const draft = callBackDraft({
    remote_e164: record.remote_e164,
    left_message: record.message_left,
  })
  const missed = record.direction === 'inbound' && record.outcome !== 'answered'
  const chip = callStateChip(record)
  const failure = hangup.error ?? drop.error ?? dismiss.error
  return (
    <div className={`phone-call${live ? ' phone-call-live' : ''}`}>
      <NavBar back={{ label: 'Home', onBack: home }} backIcon={live ? ChevronDown : undefined}>
        {live && <span className="phone-call-state">Call in progress</span>}
      </NavBar>
      <div className="phone-content phone-call-content">
        <header className="phone-portrait-head">
          <span className="phone-call-face">
            <Avatar
              id={record.agent_id}
              name={record.agent_name}
              appearance={agent?.avatar}
              size={live ? 'portrait' : 'xl'}
              presence={live ? 'oncall' : 'idle'}
            />
            {!live && missed && (
              <span className="phone-call-missed-mark">
                <PhoneMissed size={14} aria-hidden />
              </span>
            )}
          </span>
          <p className="phone-hint">
            {record.agent_name}
            {missed && !live
              ? ' missed a call from'
              : ` · Call ${record.direction === 'inbound' ? 'from' : 'to'}`}
          </p>
          <h1 className="phone-heading">{formatE164(record.remote_e164)}</h1>
          <div className="phone-badges">
            <Badge tone={live ? chip.tone : missed ? 'waiting' : 'neutral'}>
              {live ? chip.label : outcomeText(record)}
            </Badge>
            <Badge title={tierMeaning[record.tier]}>{tierLabel[record.tier] ?? record.tier}</Badge>
            <span className="phone-hint">
              {live
                ? formatDuration(callDuration(record, now))
                : changeTimeLabel(record.created_at)}
            </span>
          </div>
        </header>
        <section className="phone-section phone-call-transcript">
          <div className="phone-section-head">
            <SectionLabel>{live || !missed ? 'Transcript' : 'Message'}</SectionLabel>
            <span className="phone-hint">
              {live
                ? 'Live'
                : record.message_left || !missed
                  ? formatDuration(callDuration(record, now))
                  : null}
            </span>
          </div>
          {!live && missed && !record.message_left ? (
            <p className="phone-hint">Nobody answered.</p>
          ) : (
            <Frame>
              <Row>
                <div className="phone-call-turns">
                  <Transcript lines={lines} agentName={record.agent_name} callerLabel="Caller" />
                  {live && caption && (
                    <p className="phone-call-progress phone-hint">
                      <AudioLines size={20} aria-hidden />
                      {caption}
                    </p>
                  )}
                </div>
              </Row>
            </Frame>
          )}
          {live && (
            <p className="phone-hint phone-call-tools">
              Tools on this call: {record.tools.length ? record.tools.join(', ') : 'None'}
            </p>
          )}
        </section>
        {!live && missed && (
          <section className="phone-section">
            <SectionLabel>If you call back</SectionLabel>
            <div className="phone-well">
              <p className="phone-hint">{record.agent_name} gets this message from you:</p>
              <p className="phone-call-draft">{draft}</p>
            </div>
            <p className="phone-hint">
              {record.agent_name} prepares the call. You approve it before {record.agent_name}{' '}
              dials.
            </p>
          </section>
        )}
        {failure && (
          <p role="alert" className="phone-hint">
            {failure.message}
          </p>
        )}
      </div>
      {live ? (
        <footer className="phone-footer">
          <div className="phone-actions">
            <Button size="lg" onClick={listen.state === 'listening' ? listen.stop : listen.start}>
              <Volume2 size={20} aria-hidden />
              {listen.state === 'listening' ? 'Stop listening' : 'Listen'}
            </Button>
            <Button variant="danger-solid" size="lg" onClick={() => setConfirm('hangup')}>
              <PhoneOff size={20} aria-hidden />
              Hang up
            </Button>
          </div>
          {listen.message && (
            <p role="alert" className="phone-hint">
              {listen.message}
            </p>
          )}
          {record.tier !== 'unknown' && (
            <Button variant="link" onClick={() => setConfirm('drop')}>
              <UserRoundMinus size={20} aria-hidden />
              Drop to Unknown
            </Button>
          )}
        </footer>
      ) : (
        missed && (
          <footer className="phone-footer">
            <Button
              variant="primary"
              size="lg"
              disabled={!channelId || dismiss.isPending}
              onClick={async () => {
                if (!channelId) return
                try {
                  await dismiss.mutateAsync(callId)
                  useComposerDraft.getState().set(threadScope(channelId), draft)
                  void navigate({ to: '/c/$channelId', params: { channelId } })
                } catch {
                  /* The screen shows the failed mutation. */
                }
              }}
            >
              <Phone size={20} aria-hidden />
              Call back
            </Button>
            <Button
              variant="link"
              disabled={dismiss.isPending}
              onClick={() => dismiss.mutate(callId, { onSuccess: home })}
            >
              Dismiss
            </Button>
          </footer>
        )
      )}
      <ActionSheet
        open={confirm !== null}
        onOpenChange={(open) => {
          if (!open) setConfirm(null)
        }}
        title={confirm === 'hangup' ? 'Hang up this call?' : 'Drop this call to Unknown?'}
        description={
          confirm === 'hangup'
            ? 'The call ends for everyone.'
            : 'This cannot be undone while the call runs.'
        }
        action={{
          label: confirm === 'hangup' ? 'Hang up' : 'Drop to Unknown',
          danger: true,
          disabled: hangup.isPending || drop.isPending,
          onSelect: () => (confirm === 'hangup' ? hangup.mutate() : drop.mutate()),
        }}
      />
    </div>
  )
}
