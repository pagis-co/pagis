import type { BadgeTone } from '../primitives'

import './ConnectionStatus.css'

type ConnectionStatusPresentation = {
  label: string
  tone: 'connected' | 'warning' | 'error'
}

const CONNECTION_STATUS: Record<string, ConnectionStatusPresentation> = {
  disconnected: { label: 'Not connected', tone: 'error' },
  connecting: { label: 'Connecting', tone: 'warning' },
  connected: { label: 'Connected', tone: 'connected' },
  reauth_required: { label: 'Reconnect required', tone: 'warning' },
  unavailable: { label: 'Unavailable', tone: 'error' },
}

/** The same states as a Badge tone, for a list row that wears its state
 *  as a pill. One table, so a dot and a pill never disagree. */
const BADGE_TONE: Record<ConnectionStatusPresentation['tone'], BadgeTone> = {
  connected: 'working',
  warning: 'waiting',
  error: 'failed',
}

export const CONNECTION_STATE: Record<string, { label: string; tone: BadgeTone }> =
  Object.fromEntries(
    Object.entries(CONNECTION_STATUS).map(([status, presentation]) => [
      status,
      { label: presentation.label, tone: BADGE_TONE[presentation.tone] },
    ]),
  )

/** The user-facing state of any Connection. Keep status words and tones
 *  here so every provider card gives the same signal. */
export function ConnectionStatus({ status }: { status: string }) {
  const presentation = CONNECTION_STATUS[status] ?? {
    label: status,
    tone: 'error' as const,
  }

  return (
    <span
      className={`connection-status connection-status-${presentation.tone}`}
      role="status"
      aria-label={presentation.label}
    >
      <span className="connection-status-dot" aria-hidden="true" />
      {presentation.label}
    </span>
  )
}
