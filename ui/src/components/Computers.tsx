// The Computer tile of an Agent. Awake, the tile is a live WebRTC view
// that expands to a full-page view; asleep, it shows the last screenshot
// and a Wake button. `computer.state_changed` WS events keep state and
// preview live. An awake Computer in Home mode names the exit in use,
// and `computer.exit_changed` events keep it live.

import { useQuery } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { X } from 'lucide-react'

import type { AgentDto, ApiClient } from '../api/client'
import { fetchScreenPreview } from '../api/client'
import {
  screenPreviewKey,
  useComputer,
  useHandback,
  useSleepComputer,
  useTakeover,
  useWakeComputer,
} from '../queries'
import { Button, IconButton } from '../primitives'
import { useTakeoverCountdowns } from '../state/stores'
import { LiveScreen } from './LiveScreen'
import { ScreenFrame, holderOf } from './ScreenFrame'
import { computerStateWord } from './stateWords'

import './Computers.css'

function PreviewImage({ agent }: { agent: AgentDto }) {
  const preview = useQuery({
    queryKey: screenPreviewKey(agent.id),
    queryFn: () => fetchScreenPreview(agent.id),
    staleTime: Infinity,
  })
  // Two screenshots at most: the one the tile shows, and the one it
  // crossfades from. A refresh must never flash the empty
  // ground between two shots.
  const [shots, setShots] = useState<string[]>([])
  const shot = preview.data
  useEffect(() => {
    if (shot == null) return
    const objectUrl = URL.createObjectURL(shot)
    setShots((held) => [...held, objectUrl].slice(-2))
  }, [shot])
  // An object URL the stack dropped is free, and so is every one of
  // them once the tile goes.
  const held = useRef<string[]>([])
  useEffect(() => {
    for (const url of held.current) {
      if (!shots.includes(url)) URL.revokeObjectURL(url)
    }
    held.current = shots
  }, [shots])
  useEffect(
    () => () => {
      for (const url of held.current) URL.revokeObjectURL(url)
    },
    [],
  )

  if (shots.length === 0) {
    return <div className="computer-preview-empty">No screen yet</div>
  }
  const url = shots[shots.length - 1]
  const previous = shots.length > 1 ? shots[0] : null
  return (
    <div className="computer-preview-stack">
      {previous !== null && (
        <img
          className="computer-preview computer-preview-old"
          src={previous}
          alt=""
          aria-hidden
        />
      )}
      <img
        // A new object URL is a new element, so the fade runs again on
        // every refresh and not once per mount.
        key={url}
        className="computer-preview computer-preview-new"
        src={url}
        alt={`${agent.name}'s screen`}
      />
    </div>
  )
}

/** The 15 s handback toast: ticks down locally; the daemon's
 *  cancel or handback event removes it. */
function HandbackCountdown({ seconds }: { seconds: number }) {
  const [left, setLeft] = useState(seconds)
  useEffect(() => {
    setLeft(seconds)
    const tick = setInterval(
      () => setLeft((value) => Math.max(0, value - 1)),
      1000,
    )
    return () => clearInterval(tick)
  }, [seconds])
  return (
    <div className="handback-countdown" role="status">
      Handing control back in {left} s — any input keeps it
    </div>
  )
}

export function ComputerTile({
  api,
  agent,
}: {
  api: ApiClient
  agent: AgentDto
}) {
  const computer = useComputer(api, agent.id)
  const wake = useWakeComputer(api, agent.id)
  const sleep = useSleepComputer(api, agent.id)
  const takeover = useTakeover(api, agent.id)
  const handback = useHandback(api, agent.id)
  const state = computer.data?.state ?? 'off'
  const holder = holderOf(computer.data?.holder)
  const countdown = useTakeoverCountdowns(
    (store) => store.byAgent[agent.id],
  )
  // The full-page view covers the whole shell, so only the Expand
  // button opens it. A tile that mounts with the computer state
  // already read must draw the same screen as one that reads it after
  // the mount, and neither takes the shell from the user.
  const [expanded, setExpanded] = useState(false)
  // Fold the full-page view when the computer stops.
  useEffect(() => {
    if (state !== 'awake') setExpanded(false)
  }, [state])

  // The switch, and the way out of the full-page view. The daemon
  // holds the switch while it fills a credential: the user
  // watches, but neither side can type until the fill ends, so the
  // frame states that and offers no button.
  const screenActions = expanded ? (
    <>
      {holder === 'user' ? (
        <Button
          disabled={handback.isPending}
          onClick={() => handback.mutate()}
        >
          Hand back
        </Button>
      ) : holder === 'agent' ? (
        <Button
          variant="primary"
          disabled={takeover.isPending}
          onClick={() => takeover.mutate()}
        >
          Take over
        </Button>
      ) : null}
      {/* The stop sits beside the switch. The disk
          survives, so the next wake finds the same data. */}
      <Button
        aria-label={`Put ${agent.name}\u2019s computer to sleep`}
        disabled={sleep.isPending}
        onClick={() => sleep.mutate()}
      >
        Put to sleep
      </Button>
      <IconButton
        icon={X}
        label="Fold the screen view"
        variant="ghost"
        size="sm"
        onClick={() => setExpanded(false)}
      />
    </>
  ) : undefined

  // The one live view per open tile; the container stays mounted
  // across expand/fold so the WebRTC session survives the toggle. The
  // frame says whose computer this is and who is driving it.
  const screen =
    state === 'awake' ? (
      <div className={expanded ? 'screen-expanded' : 'screen-compact'}>
        <ScreenFrame
          agentId={agent.id}
          agentName={agent.name} avatarAppearance={agent.avatar}
          holder={holder}
          actions={screenActions}
          countdown={
            expanded && countdown !== undefined ? (
              <HandbackCountdown seconds={countdown} />
            ) : undefined
          }
        >
          <LiveScreen
            api={api}
            agentId={agent.id}
            agentName={agent.name}
            mode={!expanded ? 'compact' : holder === 'user' ? 'takeover' : 'expanded'}
            fallback={<PreviewImage agent={agent} />}
          />
        </ScreenFrame>
      </div>
    ) : (
      <PreviewImage agent={agent} />
    )

  return (
    <div className="computer-tile" data-testid="computer-tile">
      <div className="computer-tile-header">
        <span className="computer-agent">{agent.name}</span>
        <span className="computer-state">
          {computerStateWord(state, computer.data?.percent)}
        </span>
        {/* Where the pages see this Computer leave from (ADR-0029). The
            daemon writes the line, and `computer.exit_changed` events
            keep it live. */}
        {state === 'awake' && computer.data?.exit && (
          <span className="computer-exit">{computer.data.exit}</span>
        )}
        {(state === 'off' || state === 'failed') && (
          <Button
            size="sm"
            variant="primary"
            className="computer-wake"
            disabled={wake.isPending}
            onClick={() => wake.mutate()}
          >
            Wake
          </Button>
        )}
        {state === 'awake' && !expanded && (
          <Button
            size="sm"
            className="computer-expand"
            aria-label={`Expand ${agent.name}'s screen`}
            onClick={() => setExpanded(true)}
          >
            Expand
          </Button>
        )}
      </div>
      {screen}
      {wake.isError && (
        <p className="computer-error">{(wake.error as Error).message}</p>
      )}
      {state === 'failed' && computer.data?.error && (
        <p className="computer-error">{computer.data.error}</p>
      )}
      {sleep.isError && (
        <p className="computer-error">{(sleep.error as Error).message}</p>
      )}
    </div>
  )
}

/** Without Docker a computer cannot run (ADR-0024). Say so, and point
 *  at the two ways to get it. */
export function NoDocker() {
  return (
    <div className="computers-no-docker" role="status">
      <p>
        Pagis found no Docker, so no computer can start. Install Docker
        Desktop or Colima. An administrator sets its address in the
        Administration Interface.
      </p>
      <p>
        <a href="https://www.docker.com/products/docker-desktop/" target="_blank" rel="noreferrer">
          Docker Desktop
        </a>
        {' · '}
        <a href="https://github.com/abiosoft/colima" target="_blank" rel="noreferrer">
          Colima
        </a>
      </p>
    </div>
  )
}
