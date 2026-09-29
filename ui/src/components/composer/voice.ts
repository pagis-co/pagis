// The three states the voice control shows. Each state has its
// own icon, label and tooltip, so the control never leaves the user to
// guess whether the microphone is open.

import { AudioLines, LoaderCircle, Mic } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'

export type VoiceState = 'idle' | 'listening' | 'transcribing'

export interface VoiceLook {
  icon: LucideIcon
  /** The accessible name and the tooltip of the control. */
  label: string
}

const LOOKS: Record<VoiceState, VoiceLook> = {
  idle: { icon: Mic, label: 'Hold to talk' },
  listening: { icon: AudioLines, label: 'Listening' },
  transcribing: { icon: LoaderCircle, label: 'Transcribing' },
}

/** The state of the control for the utterance in progress: none is
 * idle, a held utterance is listening, and a released one transcribes
 * until its final transcript lands. */
export function voiceState(utterance: { held: boolean } | null): VoiceState {
  if (utterance === null) return 'idle'
  return utterance.held ? 'listening' : 'transcribing'
}

export function voiceLook(state: VoiceState): VoiceLook {
  return LOOKS[state]
}
