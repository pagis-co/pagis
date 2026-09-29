import {
  Component,
  Suspense,
  lazy,
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from 'react'
import type { ReactNode } from 'react'
import { create } from 'zustand'
import {
  appearanceKey,
  spriteAsset,
  spriteCatalog,
  type SpriteAppearance,
} from './catalog'
import type { SpriteClip, SpriteExpression } from './motion'
import './sprite.css'

const Scene = lazy(() => import('./SpriteScene'))
const useMotionOwner = create<{
  owner: symbol | null
  claims: Map<symbol, number>
  claim: (owner: symbol, priority: number | null) => void
}>((set) => ({
  owner: null,
  claims: new Map(),
  claim: (id, priority) =>
    set((state) => {
      const claims = new Map(state.claims)
      if (priority === null) claims.delete(id)
      else claims.set(id, priority)
      let owner: symbol | null = null
      let highest = 0
      for (const [id, rank] of claims)
        if (rank >= highest) {
          highest = rank
          owner = id
        }
      return { claims, owner }
    }),
}))
function subscribeMotion(update: () => void) {
  const query = window.matchMedia('(prefers-reduced-motion: reduce)')
  query.addEventListener('change', update)
  document.addEventListener('visibilitychange', update)
  return () => {
    query.removeEventListener('change', update)
    document.removeEventListener('visibilitychange', update)
  }
}
class RenderBoundary extends Component<
  { children: ReactNode; poster: ReactNode },
  { failed: boolean }
> {
  state = { failed: false }
  static getDerivedStateFromError() {
    return { failed: true }
  }
  render() {
    return this.state.failed ? this.props.poster : this.props.children
  }
}
export interface SpriteAvatarProps {
  name: string
  appearance: SpriteAppearance
  clip?: SpriteClip
  expression?: SpriteExpression
  animate?: boolean
  hover?: boolean
  interactive?: boolean
  className?: string
}
export function SpriteAvatar({
  name,
  appearance,
  clip = 'Idle',
  expression,
  animate = false,
  hover = false,
  interactive = false,
  className = '',
}: SpriteAvatarProps) {
  const [sceneReady, setSceneReady] = useState(false)
  const onReady = useCallback(() => setSceneReady(true), [])
  const host = useRef<HTMLSpanElement>(null)
  const identity = useRef(Symbol())
  const owner = useMotionOwner((state) => state.owner)
  const [visible, setVisible] = useState(false)
  const [portrait, setPortrait] = useState<{ key: string; url: string } | null>(
    null,
  )
  const key = appearanceKey(appearance)
  const motion = useSyncExternalStore(
    subscribeMotion,
    () =>
      !window.matchMedia('(prefers-reduced-motion: reduce)').matches &&
      !document.hidden,
    () => false,
  )
  useEffect(() => {
    if (!host.current || typeof IntersectionObserver === 'undefined') return
    const observer = new IntersectionObserver(([entry]) =>
      setVisible(entry.isIntersecting),
    )
    observer.observe(host.current)
    return () => observer.disconnect()
  }, [])
  useEffect(() => {
    if (!visible || document.hidden) return
    let current = true
    const timer = setTimeout(() => {
      void import('./rendering')
        .then(({ spritePortrait }) => spritePortrait(appearance))
        .then((url) => {
          if (current) setPortrait({ key, url })
        })
        .catch(() => {
          /* The exported portrait remains visible if WebGL is unavailable. */
        })
    }, 100)
    return () => {
      current = false
      clearTimeout(timer)
    }
  }, [key, visible, motion, appearance])
  useEffect(() => {
    if (!animate || !visible || !motion) return
    const id = identity.current
    useMotionOwner.getState().claim(id, hover ? 2 : 1)
    return () => useMotionOwner.getState().claim(id, null)
  }, [animate, hover, visible, motion])
  const definition = spriteCatalog[appearance.sprite]
  const playing = animate && motion && visible && owner === identity.current
  useEffect(() => {
    if (!playing) setSceneReady(false)
  }, [playing])
  const poster = (
    <img
      className={`sprite-portrait${playing && sceneReady ? ' sprite-portrait-hidden' : ''}`}
      src={
        portrait?.key === key
          ? portrait.url
          : spriteAsset(definition.presets[appearance.preset].portrait)
      }
      alt=""
    />
  )
  return (
    <span
      ref={host}
      className={`sprite-avatar ${className}`}
      role="img"
      aria-label={`${name}, ${definition.label} avatar`}
      data-sprite={appearance.sprite}
      data-motion={playing ? clip : 'static'}
    >
      {poster}
      <RenderBoundary
        key={appearance.sprite}
        poster={
          <img
            className="sprite-portrait sprite-fallback"
            src={
              portrait?.key === key
                ? portrait.url
                : spriteAsset(definition.presets[appearance.preset].portrait)
            }
            alt=""
          />
        }
      >
        {playing && (
          <Suspense fallback={null}>
            <Scene
              onReady={onReady}
              appearance={appearance}
              clip={clip}
              expression={expression}
              interactive={interactive}
            />
          </Suspense>
        )}
      </RenderBoundary>
    </span>
  )
}
