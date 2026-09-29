// The sound setting: four cues, off until the user asks for
// them. The choice lives in this browser, beside the theme, because it
// belongs to the machine the user listens on and not to the account.

import { create } from 'zustand'

import { CuePlayer, type CueName } from '../sound/cues'

export const SOUND_KEY = 'pagis-sound'

/** Read the stored choice. Anything but `on` means off, so a cleared
 *  or damaged value is silence. */
export function storedSound(): boolean {
  try {
    return window.localStorage.getItem(SOUND_KEY) === 'on'
  } catch {
    return false
  }
}

export interface SoundState {
  enabled: boolean
  setEnabled: (enabled: boolean) => void
}

export const useSound = create<SoundState>((set) => ({
  enabled: storedSound(),
  setEnabled: (enabled) => {
    try {
      window.localStorage.setItem(SOUND_KEY, enabled ? 'on' : 'off')
    } catch {
      // A browser that refuses storage still changes the session.
    }
    set({ enabled })
  },
}))

/** The request kinds that ask the user to allow something. A form or a
 *  choice is a question, not an approval, so it stays silent. */
const APPROVAL_KINDS = ['tool_action', 'credential_action', 'computer_safety']

/**
 * The cue one firehose frame earns, or `null` for the frames that
 * carry no news the ear needs. The shell folds every frame through
 * this function, so the four moments are named in one place.
 */
export function cueForFrame(
  type: string,
  payload: { kind?: string; to?: string },
): CueName | null {
  if (type === 'request.created') {
    return payload.kind !== undefined && APPROVAL_KINDS.includes(payload.kind)
      ? 'approval'
      : null
  }
  if (type === 'call.placed') return 'ringing'
  if (type === 'call.answered') return 'connected'
  if (type === 'run.state_changed' && payload.to === 'failed') return 'failed'
  return null
}

const player = new CuePlayer()

/** Play a cue if the setting is on. Every caller goes through here, so
 *  the setting is the one gate on sound in the product. */
export function playCue(name: CueName): void {
  if (!useSound.getState().enabled) return
  player.play(name)
}
