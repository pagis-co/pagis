import type { Presence } from '../primitives/avatar'

export type SpriteClip =
  'Idle' | 'Working' | 'Waiting' | 'NeedsInput' | 'Celebrate' | 'Error'
export type SpriteExpression =
  'Neutral' | 'Blink' | 'Smile' | 'Surprise' | 'Concern'
export interface Motion {
  clip: SpriteClip
  expression: SpriteExpression
  animate: boolean
}

/** Status takes priority over a playful hover. No model call selects motion. */
export function avatarMotion({
  presence,
  online,
  hover = false,
  reducedMotion = false,
}: {
  presence: Presence
  online: boolean
  hover?: boolean
  reducedMotion?: boolean
}): Motion {
  const clip =
    presence === 'working'
      ? 'Working'
      : presence === 'waiting'
        ? 'NeedsInput'
        : presence === 'oncall'
          ? 'Idle'
          : hover
            ? 'Celebrate'
            : 'Waiting'
  return {
    clip,
    expression:
      clip === 'Celebrate'
        ? 'Smile'
        : clip === 'NeedsInput'
          ? 'Surprise'
          : 'Neutral',
    animate: online && !reducedMotion,
  }
}
