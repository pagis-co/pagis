// The `call` block (ADR-0022). The daemon mints one
// of these in the Thread that sent the Agent to make the call.
//
// One card, running or settled: the agent's face, the number, the
// contact tier, a state chip in the semantic appearance (blue on a connected
// call, red on a failed one), and the duration. The head of the card
// is the header line of the frame. It opens the call inspector, and it
// is the only way back to a live Call.
//
// Settled, the card keeps the two answers apart, as ADR-0020 requires:
// the outcome, then the ended reason with the duration and the
// classification. A call that never went through says why in plain
// words. The recording plays in the card's own scrubber, and the
// transcript opens inline under the same card; a long one opens in the
// inspector instead.
//
// Every fact comes from the Call record. The `call.transcript` events
// add only the lines that arrive after the record was read.

import { useEffect, useState } from 'react'
import { ChevronDown, ChevronUp } from 'lucide-react'

import type { ApiClient, CallDto } from '../api/client'
import { fetchArtifactBlob } from '../api/client'
import { Avatar, Badge, Button } from '../primitives'
import type { BadgeTone } from '../primitives'
import { useCall, useAgents } from '../queries'
import { useCallInspector, useCallTranscripts, selectCallLines } from '../state/stores'
import { Card, CardBody } from './Card'
import {
  LONG_TRANSCRIPT,
  callDuration,
  callFailed,
  callStateChip,
  classificationText,
  endedReason,
  failureText,
  formatDuration,
  formatE164,
  lastSaid,
  mergeTranscript,
  outcomeText,
  tierLabel,
  whoLine,
} from './call'
import { Transcript } from './Transcript'
import { WaveScrubber } from './WaveScrubber'

import './call.css'

/** The wall clock, ticking once a second while a Call runs. */
export function useNow(live: boolean): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!live) return
    setNow(Date.now())
    const tick = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(tick)
  }, [live])
  return now
}

const TIER_TONE: Record<string, BadgeTone> = {
  owner: 'accent',
  trusted: 'working',
  unknown: 'waiting',
}

export function TierChip({ tier }: { tier: string }) {
  return (
    <Badge tone={TIER_TONE[tier] ?? 'neutral'} className={`call-tier call-tier-${tier}`}>
      {tierLabel[tier] ?? tier}
    </Badge>
  )
}

/** The recording, played where it belongs: on the card that owns it. */
function Recording({
  artifactId,
  durationMs,
}: {
  artifactId: string
  durationMs: number
}) {
  const [blob, setBlob] = useState<Blob | null>(null)
  const [failed, setFailed] = useState(false)
  useEffect(() => {
    let canceled = false
    fetchArtifactBlob(artifactId)
      .then((loaded) => {
        if (!canceled) setBlob(loaded)
      })
      .catch(() => {
        if (!canceled) setFailed(true)
      })
    return () => {
      canceled = true
    }
  }, [artifactId])
  if (failed) {
    return <p className="call-recording-missing">The recording is unavailable.</p>
  }
  if (blob === null) return null
  return (
    <WaveScrubber
      blob={blob}
      label="The recording of the call"
      fallbackMs={durationMs}
    />
  )
}

/** The head of the card: who, what tier, what state, how long. It is
 *  the header line of the frame, and the way into the inspector. */
function Head({ appearance, call, onOpen }: { appearance?: import('../avatars/catalog').SpriteAppearance; call: CallDto; onOpen: () => void }) {
  const live = call.state !== 'ended'
  const lines = useCallTranscripts(selectCallLines(call.id))
  const now = useNow(live)
  const said = lastSaid(mergeTranscript(call.transcript, lines))
  const chip = callStateChip(call)
  return (
    <Button
      variant="ghost"
      shape="row"
      className="block-strip call-strip"
      onClick={onOpen}
      data-testid="call-strip"
    >
      <Avatar
        id={call.agent_id}
        name={call.agent_name}
        appearance={appearance}
        size="sm"
        presence={live ? 'oncall' : 'none'}
      />
      <span className="call-strip-text">
        <strong>{whoLine(call)}</strong>
        {said !== null && <span className="call-strip-said">{said}</span>}
      </span>
      <span className="call-strip-status">
        <TierChip tier={call.tier} />
        <Badge tone={chip.tone}>
          {chip.label} · {formatDuration(callDuration(call, now))}
        </Badge>
      </span>
      <span className="block-strip-open">{live ? 'Listen live' : 'Open'}</span>
    </Button>
  )
}

/** What a settled Call left behind, under the same card head. */
function Settled({ call, onOpen, appearance }: { call: CallDto; onOpen: () => void; appearance?: import('../avatars/catalog').SpriteAppearance }) {
  const [openTranscript, setOpenTranscript] = useState(false)
  const lines = mergeTranscript(call.transcript, [])
  const long = lines.length > LONG_TRANSCRIPT
  const failed = callFailed(call)
  const classification = classificationText(call)
  return (
    <Card className="call-settled" data-testid="call-settled">
      <Head appearance={appearance} call={call} onOpen={onOpen} />
      <CardBody className={failed ? 'call-outcome call-outcome-failed' : 'call-outcome'}>
        <strong>{outcomeText(call)}</strong>
        {failed && <span className="call-failure">{failureText(call)}</span>}
        <span>
          Ended because {endedReason(call.ended_reason)} ·{' '}
          {formatDuration(callDuration(call, Date.now()))}
          {classification != null && <> · {classification}</>}
        </span>
      </CardBody>
      {call.recording_artifact_id != null && (
        <CardBody>
          <Recording
            artifactId={call.recording_artifact_id}
            durationMs={callDuration(call, Date.now())}
          />
        </CardBody>
      )}
      {lines.length > 0 && (
        <CardBody className="call-transcript-body">
          {long ? (
            <Button variant="ghost" className="call-transcript-toggle" onClick={onOpen}>
              Read the transcript ({lines.length} lines)
            </Button>
          ) : (
            <>
              <Button
                variant="ghost"
                className="call-transcript-toggle"
                aria-expanded={openTranscript}
                onClick={() => setOpenTranscript((open) => !open)}
              >
                {openTranscript ? (
                  <ChevronUp size={14} aria-hidden focusable="false" />
                ) : (
                  <ChevronDown size={14} aria-hidden focusable="false" />
                )}
                {openTranscript ? 'Hide the transcript' : 'Read the transcript'}
              </Button>
              {openTranscript && (
                <Transcript lines={lines} agentName={call.agent_name} />
              )}
            </>
          )}
        </CardBody>
      )}
    </Card>
  )
}

/** The block, as the Thread renders it. */
export function CallBlock({ callId, api }: { callId: string; api: ApiClient }) {
  const call = useCall(api, callId)
  const agents = useAgents(api)
  const open = useCallInspector((state) => state.open)
  if (call.data === undefined) {
    return (
      <Card className="call-block">
        <CardBody className="call-strip-waiting" data-testid="call-waiting">
          {call.isError ? 'That call is not on record.' : 'Placing the call…'}
        </CardBody>
      </Card>
    )
  }
  const record = call.data
  const appearance = agents.data?.find((agent) => agent.id === record.agent_id)?.avatar
  return (
    <div className="call-block" data-testid="call-block">
      {record.direction === 'inbound' && (
        <p className="call-inbound-note">
          {record.agent_name} answered its desk line{' '}
          {formatE164(record.own_e164)} under its standing brief.
        </p>
      )}
      {record.state === 'ended' ? (
        <Settled appearance={appearance} call={record} onOpen={() => open(callId)} />
      ) : (
        <Card className="call-live">
          <Head appearance={appearance} call={record} onOpen={() => open(callId)} />
        </Card>
      )}
    </div>
  )
}
