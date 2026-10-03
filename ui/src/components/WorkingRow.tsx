// The Agent at work: one live row at the end of the reading
// column. It carries the presence ring, the step the Run is on, the
// time it has worked, a look at the desk, and Stop. The tool loop
// writes no "Thinking…" message into the column.

import { useEffect, useState } from 'react'
import { Square } from 'lucide-react'

import type { ApiClient } from '../api/client'
import { Avatar, Button } from '../primitives'
import { useCancelRun, useRunSteps, useScreenPreviewUrl } from '../queries'
import type { TimelineRow } from '../timeline'

import './WorkingRow.css'

/** `0:42`, the time the Run has worked. */
export function formatTimer(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000))
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`
}

/** The seconds since the Run started, one tick at a time. */
function useElapsed(since: number): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [])
  return now - since
}

/** The last screen of the Agent's desk, as a tile that opens it. */
function DeskThumbnail({
  agentId,
  onOpen,
}: {
  agentId: string
  onOpen: () => void
}) {
  const preview = useScreenPreviewUrl(agentId)
  return (
    <Button
      variant="link"
      className="working-desk"
      aria-label="Open the desk"
      onClick={onOpen}
    >
      <span className="working-desk-screen">
        {preview !== null && <img src={preview} alt="" />}
      </span>
    </Button>
  )
}

export function WorkingRow({
  api,
  row,
  agentName,
  agentAppearance,
  onOpenDesk,
}: {
  api: ApiClient
  /** The live progress row of the Run. */
  row: TimelineRow
  agentName: string
  agentAppearance?: import('../avatars/catalog').SpriteAppearance
  /** Opens the Desk panel; absent where no panel sits beside the row. */
  onOpenDesk?: () => void
}) {
  const steps = useRunSteps(api, row.runId)
  // A cancel the daemon accepted holds until the Run ends and the row
  // goes; a failed one gives Stop back.
  const cancel = useCancelRun(api)
  const stopping = cancel.isPending || cancel.isSuccess
  const elapsed = useElapsed(row.createdAt)
  const agentId = row.authorAgentId
  const count = steps.data?.steps.length ?? 0
  const reflecting = row.text === 'Updating memory'
  return (
    <div
      className={`working${reflecting ? ' working-reflecting' : ''}`}
      data-testid={reflecting ? 'reflecting-row' : 'working-row'}
    >
      <Avatar
        active
        id={agentId ?? 'agent'}
        name={agentName}
        appearance={agentAppearance}
        size="md"
        presence={reflecting ? 'none' : 'working'}
      />
      <span className="working-lines">
        <span className="working-line">
          {reflecting ? (
            <span className="working-step">Updating memory</span>
          ) : (
            <>
              <strong>{agentName} is working</strong>
              <span className="working-step"> · {row.text}</span>
            </>
          )}
        </span>
        {!reflecting && (
          <span className="working-time">
            {count > 0 && `step ${count} of the Run · `}
            {formatTimer(elapsed)}
          </span>
        )}
      </span>
      {!reflecting && agentId !== null && onOpenDesk !== undefined && (
        <DeskThumbnail agentId={agentId} onOpen={onOpenDesk} />
      )}
      {row.runId != null && (
        <Button
          size="sm"
          variant="outline"
          disabled={stopping}
          onClick={() => cancel.mutate(row.runId as string)}
        >
          <Square size={14} aria-hidden focusable="false" />
          {stopping ? 'Stopping…' : 'Stop'}
        </Button>
      )}
    </div>
  )
}
