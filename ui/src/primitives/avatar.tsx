import { useEffect, useRef, useState, type ReactNode } from 'react'
import { SpriteAvatar } from '../avatars/SpriteAvatar'
import { defaultAppearance, type SpriteAppearance } from '../avatars/catalog'
import { useAgentAppearance } from '../avatars/AvatarRoster'
import { useLiveAvatarMotion } from '../avatars/liveMotion'

import { cx } from './cx'
import './avatar.css'

export type Presence = 'working' | 'waiting' | 'oncall' | 'idle' | 'none'
export type AvatarSize = 'sm' | 'md' | 'lg' | 'xl' | 'face' | 'portrait'

/** The letter drawn on the disc. */
export function avatarInitial(name: string): string {
  const first = [...name.trim()][0]
  return first === undefined ? '?' : first.toUpperCase()
}

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

/** The user's own face. The user is not an Agent, so it takes none of
 *  the Agent hues: a hashed hue can land on the ground it is drawn on
 *  and disappear, and the owner reads as one person in every place
 *  that shows them. The name gives the initial, so every place passes
 *  the same one. */
export function OwnerAvatar({
  name,
  size = 'md',
  className,
  outlined = false,
}: {
  /** The name the initial comes from. */
  name: string
  size?: AvatarSize
  className?: string
  outlined?: boolean
}) {
  return (
    <span
      className={cx(
        'ui-avatar',
        `ui-avatar-${size}`,
        'ui-avatar-owner',
        outlined && 'ui-avatar-owner-outlined',
        className,
      )}
      aria-hidden
    >
      {avatarInitial(name)}
    </span>
  )
}

export interface AvatarGroupProps {
  /** The faces, in the order they overlap. */
  children: ReactNode
  className?: string
}

/** Two or three faces of a group, each one over the last. */
export function AvatarGroup({ children, className }: AvatarGroupProps) {
  return <span className={cx('ui-avatar-group', className)}>{children}</span>
}
