// The frame around a live screen. A bare video says nothing
// about whose computer it is or who is driving it. The frame is window
// chrome: the agent's face and name, who holds the switch, the acts on
// the screen, and the handback countdown, which belongs on the screen
// it hands back and not in a toast somewhere else.

import type { ReactNode } from 'react'

import { Avatar } from '../primitives'
import { controlWord } from './stateWords'

import './ScreenFrame.css'

/** Who may type on the computer. */
export type Holder = 'agent' | 'user' | 'daemon'

/** The daemon types the holder as a plain string. A value the UI does
 *  not know reads as the agent's, which is the state that offers the
 *  user the switch and forwards no input. */
export function holderOf(value: string | null | undefined): Holder {
  return value === 'user' || value === 'daemon' ? value : 'agent'
}

export function ScreenFrame({
  agentId,
  agentName,
  avatarAppearance,
  holder,
  countdown,
  actions,
  children,
}: {
  agentId: string
  avatarAppearance?: import('../avatars/catalog').SpriteAppearance
  agentName: string
  holder: Holder
  /** The handback countdown, in seconds; absent while none runs. */
  countdown?: ReactNode
  /** The acts on this screen: take over, hand back, fold. */
  actions?: ReactNode
  children: ReactNode
}) {
  return (
    <div
      className={`screen-frame screen-frame-${holder}`}
      data-testid="screen-frame"
      data-holder={holder}
    >
      <div className="screen-frame-bar">
        <Avatar id={agentId} name={agentName} appearance={avatarAppearance} size="sm" />
        <span className="screen-frame-title">{agentName}'s computer</span>
        <span className="screen-frame-holder" role="status">
          {controlWord(holder, agentName)}
        </span>
        {actions !== undefined && (
          <span className="screen-frame-actions">{actions}</span>
        )}
      </div>
      <div className="screen-frame-body">
        {countdown}
        {children}
      </div>
    </div>
  )
}
