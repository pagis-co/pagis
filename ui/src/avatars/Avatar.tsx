import { useEffect, useRef, useState } from 'react'
import { SpriteAvatar } from './SpriteAvatar'
import { defaultAppearance, type SpriteAppearance } from './catalog'
import { useAgentAppearance } from './AvatarRoster'
import { useLiveAvatarMotion } from './liveMotion'
import { cx, type AvatarSize, type Presence } from '../primitives'
import '../primitives/avatar.css'

export interface AvatarProps {
  id: string
  name: string
  appearance?: SpriteAppearance
  size?: AvatarSize
  presence?: Presence
  playful?: boolean
  active?: boolean
  className?: string
}

/** Every surface reads the same saved Agent appearance. */
export function Avatar({
  id,
  name,
  appearance,
  size = 'md',
  presence = 'none',
  playful = false,
  active = false,
  className,
}: AvatarProps) {
  const saved = useAgentAppearance(id)
  const [hover, setHover] = useState(false)
  const host = useRef<HTMLSpanElement>(null)
  useEffect(() => {
    if (!playful) return
    const button = host.current?.closest('button, a')
    const enter = () => setHover(true)
    const leave = () => setHover(false)
    button?.addEventListener('focus', enter)
    button?.addEventListener('blur', leave)
    return () => {
      button?.removeEventListener('focus', enter)
      button?.removeEventListener('blur', leave)
    }
  }, [playful])
  const motion = useLiveAvatarMotion(id, presence, hover)
  return (
    <span
      ref={host}
      className={cx(
        'ui-avatar',
        `ui-avatar-${size}`,
        presence === 'none' ? null : `ui-avatar-presence-${presence}`,
        !motion.animate && 'ui-avatar-static',
        className,
      )}
      data-presence={presence}
      aria-hidden
      onPointerEnter={() => {
        if (playful) setHover(true)
      }}
      onPointerLeave={() => setHover(false)}
    >
      <SpriteAvatar
        name={name}
        appearance={appearance ?? saved ?? defaultAppearance()}
        {...motion}
        animate={motion.animate && (active || hover)}
        hover={hover}
      />
    </span>
  )
}

