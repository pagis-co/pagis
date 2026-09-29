// The call inspector (ADR-0022): a tenant of the inspector slot,
// beside the Desk Panel, the mail inspector and the thread.
//
// It carries the headline — who is on the call, the Agent and its
// number, the elapsed time and the Trust Tier with what the tier means
// — the purpose from the Call Brief, listen-live, the running
// transcript, the tools the call may use, and the line that says an
// action needing approval waits for the end of the call.
//
// It stays open across navigation, because listening is the reason it
// exists. It is transient: it exists only while a Call is live or a
// settled Call is open, so it never competes for the slot at rest.
//
// Two controls act on the call: Hang up, and Drop to Unknown behind a
// confirmation, because ADR-0021 lets a tier fall by hand only and the
// fall cannot be undone while the call runs.

import { useState } from 'react'
import { X } from 'lucide-react'

import type { ApiClient } from '../api/client'
import {
  callDuration,
  classificationText,
  endedReason,
  formatDuration,
  formatE164,
  mergeTranscript,
  outcomeText,
  tierMeaning,
  whoLine,
} from '../blocks/call'
import { TierChip, useNow } from '../blocks/CallBlock'
import { ListenLive } from '../blocks/ListenLive'
import { Transcript } from '../blocks/Transcript'
import { Button, IconButton } from '../primitives'
import { errorMessage, useCall, useDropCallTier, useHangUpCall } from '../queries'
import { selectCallLines, useCallTranscripts } from '../state/stores'

import '../blocks/call.css'

/** The controls of a live call. Drop to Unknown asks first. */
function Controls({ api, callId }: { api: ApiClient; callId: string }) {
  const hangUp = useHangUpCall(api, callId)
  const drop = useDropCallTier(api, callId)
  const [confirming, setConfirming] = useState(false)
  return (
    <div className="call-controls">
      <Button
        variant="danger"
        className="call-hang-up"
        disabled={hangUp.isPending}
        onClick={() => hangUp.mutate()}
      >
        Hang up
      </Button>
      {confirming ? (
        <span className="call-confirm">
          <span>Drop this call to Unknown? It cannot be undone on this call.</span>
          <Button
            variant="danger"
            disabled={drop.isPending}
            onClick={() => drop.mutate(undefined, { onSettled: () => setConfirming(false) })}
          >
            Drop it
          </Button>
          <Button onClick={() => setConfirming(false)}>Keep the tier</Button>
        </span>
      ) : (
        <Button onClick={() => setConfirming(true)}>Drop to Unknown</Button>
      )}
      {hangUp.isError && (
        <p className="call-error" role="alert">
          {errorMessage(hangUp.error, 'That call did not hang up.')}
        </p>
      )}
      {drop.isError && (
        <p className="call-error" role="alert">
          {errorMessage(drop.error, 'The tier did not drop.')}
        </p>
      )}
    </div>
  )
}

export function CallInspector({
  api,
  callId,
  onClose,
}: {
  api: ApiClient
  callId: string
  onClose: () => void
}) {
  const call = useCall(api, callId)
  const buffered = useCallTranscripts(selectCallLines(callId))
  const live = call.data !== undefined && call.data.state !== 'ended'
  const now = useNow(live)

  if (call.data === undefined) {
    return (
      <div className="call-inspector" data-testid="call-inspector-waiting">
        <header className="call-inspector-header">
          <h2>Call</h2>
          <IconButton
            icon={X}
            label="Close the call"
            variant="ghost"
            onClick={onClose}
          />
        </header>
        <p className="call-inspector-waiting">
          {call.isError ? 'That call is not on record.' : 'Placing the call…'}
        </p>
      </div>
    )
  }

  const record = call.data
  const lines = mergeTranscript(record.transcript, buffered)
  return (
    <div className="call-inspector" data-testid="call-inspector">
      <header className="call-inspector-header">
        <h2>{live ? 'Call in progress' : 'Call'}</h2>
        <IconButton
          icon={X}
          label="Close the call"
          variant="ghost"
          onClick={onClose}
        />
      </header>
      <div className="call-inspector-body">
        <div className="call-headline">
          <strong>{whoLine(record)}</strong>
          <span className="call-headline-agent">
            {record.agent_name} · {formatE164(record.own_e164)}
          </span>
          <span className="call-headline-state">
            {live && <span className="call-live-dot" aria-hidden />}
            <span className="call-clock">{formatDuration(callDuration(record, now))}</span>
            <TierChip tier={record.tier} />
          </span>
        </div>
        <p className="call-tier-meaning">{tierMeaning[record.tier] ?? record.tier}</p>
        <p className="call-purpose">{record.purpose}</p>

        {live ? (
          <>
            <ListenLive callId={callId} />
            <Transcript lines={lines} agentName={record.agent_name} />
            <Controls api={api} callId={callId} />
            <p className="call-tools">
              {record.tools.length === 0
                ? 'This call binds no tools.'
                : `Tools on this call: ${record.tools.join(', ')}.`}{' '}
              An action that needs your approval waits for the end of the call.
            </p>
          </>
        ) : (
          <>
            <div className="call-outcome">
              <strong>{outcomeText(record)}</strong>
              <span>
                Ended because {endedReason(record.ended_reason)} ·{' '}
                {formatDuration(callDuration(record, now))}
                {classificationText(record) != null && (
                  <> · {classificationText(record)}</>
                )}
              </span>
            </div>
            <Transcript lines={lines} agentName={record.agent_name} />
          </>
        )}
      </div>
    </div>
  )
}
