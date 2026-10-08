import { X, Moon, MousePointer2 } from 'lucide-react'
import { useState } from 'react'
import type { AgentDto, ApiClient } from '../../api/client'
import { Avatar, Button, IconButton } from '../../primitives'
import { useComputer, useHandback, useSleepComputer, useTakeover } from '../../queries'
import { useTakeoverCountdowns } from '../../state/stores'
import { agentPresence, usePresence } from '../../state/presence'
import { LiveScreen } from '../LiveScreen'
import { ScreenFrame, holderOf } from '../ScreenFrame'
import { computerStateWord, controlWord } from '../stateWords'
import './desk-phone.css'

export function DeskPhone({
  api,
  agent,
  onClose,
}: {
  api: ApiClient
  agent: AgentDto
  onClose: () => void
}) {
  const [keyboardTarget, setKeyboardTarget] = useState<HTMLDivElement | null>(null)
  const computer = useComputer(api, agent.id)
  const sleep = useSleepComputer(api, agent.id)
  const take = useTakeover(api, agent.id)
  const hand = useHandback(api, agent.id)
  const holder = holderOf(computer.data?.holder)
  const countdown = useTakeoverCountdowns((state) => state.byAgent[agent.id])
  const presence = usePresence((state) => agentPresence(state, agent.id))
  const waiting = usePresence(
    (state) =>
      Object.values(state.runs).find(
        (run) =>
          run.agentId === agent.id &&
          ['waiting_for_user', 'waiting_for_approval'].includes(run.state),
      )?.caption,
  )
  const state = computer.data?.state ?? 'off'
  return (
    <div className="desk-phone" data-theme="dark">
      <header className="desk-phone-header">
        <Avatar
          id={agent.id}
          name={agent.name}
          appearance={agent.avatar}
          size="md"
          presence={presence}
        />
        <div className="phone-row-copy">
          <strong>{agent.name}'s desk</strong>
          <span>
            {computer.isPending
              ? 'Reading the desk…'
              : computer.isError
                ? 'Could not read the desk'
                : computerStateWord(state, computer.data?.percent)}
          </span>
        </div>
        <IconButton icon={X} label="Fold the screen view" variant="ghost" onClick={onClose} />
      </header>
      <div className="desk-phone-stage">
        {state === 'awake' ? (
          <ScreenFrame
            agentId={agent.id}
            agentName={agent.name}
            avatarAppearance={agent.avatar}
            holder={holder}
          >
            <LiveScreen
              api={api}
              agentId={agent.id}
              agentName={agent.name}
              keyboardTarget={keyboardTarget}
              mode={holder === 'user' ? 'takeover' : 'expanded'}
              fallback={<p>The screen could not connect.</p>}
            />
          </ScreenFrame>
        ) : (
          <div className="desk-phone-asleep">
            {computer.isPending ? (
              <p role="status">Reading the desk…</p>
            ) : computer.isError ? (
              <>
                <p role="alert">Could not read the desk.</p>
                <Button onClick={() => void computer.refetch()}>Try again</Button>
              </>
            ) : (
              <>
                <Moon size={20} aria-hidden />
                <p>
                  {computerStateWord(state, computer.data?.percent)}. It wakes when {agent.name}{' '}
                  starts work.
                </p>
              </>
            )}
          </div>
        )}
        <div className="desk-phone-control">
          <span className={`desk-phone-holder desk-phone-holder-${holder}`}>
            {controlWord(holder, agent.name)}
          </span>
          {holder === 'user' && countdown !== undefined ? (
            <p>Handing control back in {countdown} s — any input keeps it</p>
          ) : (
            waiting && <p className="phone-hint">{waiting}</p>
          )}
        </div>
      </div>
      <footer className="desk-phone-footer">
        {state === 'awake' && (
          <>
            {holder === 'user' ? (
              <div className="phone-actions">
                <div ref={setKeyboardTarget} className="desk-phone-keyboard" />
                <Button size="lg" disabled={hand.isPending} onClick={() => hand.mutate()}>
                  Hand back
                </Button>
              </div>
            ) : holder === 'agent' ? (
              <Button
                size="lg"
                variant="primary"
                disabled={take.isPending}
                onClick={() => take.mutate()}
              >
                <MousePointer2 size={20} aria-hidden />
                Take over
              </Button>
            ) : null}
            <Button variant="link" disabled={sleep.isPending} onClick={() => sleep.mutate()}>
              <Moon size={20} aria-hidden />
              Put to sleep
            </Button>
          </>
        )}
        {[take, hand, sleep].some((mutation) => mutation.isError) && (
          <p role="alert">The computer could not be changed. Try again.</p>
        )}
      </footer>
    </div>
  )
}
