import type { ReactNode } from 'react'

import { cx } from './cx'
import './avatar.css'

export type Presence = 'working' | 'waiting' | 'oncall' | 'idle' | 'none'
export type AvatarSize = 'sm' | 'md' | 'lg' | 'xl' | 'face' | 'portrait'

/** The letter drawn on the disc. */
export function avatarInitial(name: string): string {
  const first = [...name.trim()][0]
  return first === undefined ? '?' : first.toUpperCase()
}

/** A Person's initial, with no Agent appearance. */
export function PersonAvatar({
  name,
  size = 'md',
  className,
}: {
  name: string
  size?: AvatarSize
  className?: string
}) {
  return (
    <span
      className={cx(
        'ui-avatar',
        `ui-avatar-${size}`,
        'ui-avatar-person',
        className,
      )}
      aria-hidden
    >
      {avatarInitial(name)}
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
    <PersonAvatar
      name={name}
      size={size}
      className={cx(
        'ui-avatar-owner',
        outlined && 'ui-avatar-owner-outlined',
        className,
      )}
    />
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
