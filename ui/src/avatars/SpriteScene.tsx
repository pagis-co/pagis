import { Suspense, use, useEffect, useState } from 'react'
import { Canvas, useFrame, useThree } from '@react-three/fiber'
import { OrbitControls } from '@react-three/drei'
import { createSprite } from './sprite'
import { loadSprite, spriteStage } from './rendering'
import { spriteCatalog, type SpriteAppearance } from './catalog'
import type { SpriteClip, SpriteExpression } from './motion'

interface Props {
  onReady: () => void
  appearance: SpriteAppearance
  clip: SpriteClip
  expression?: SpriteExpression
  interactive?: boolean
}
function Character({ appearance, clip, expression, onReady }: Props) {
  const asset = use(loadSprite(appearance.sprite))
  const [character, setCharacter] = useState<ReturnType<
    typeof createSprite
  > | null>(null)
  const { scene, gl } = useThree()
  useEffect(() => {
    const stage = spriteStage(gl, spriteCatalog[appearance.sprite])
    const current = createSprite(asset, spriteCatalog[appearance.sprite])
    scene.add(stage.scene)
    scene.environment = stage.scene.environment
    scene.environmentIntensity = stage.scene.environmentIntensity
    setCharacter(current)
    onReady()
    return () => {
      scene.remove(stage.scene)
      scene.environment = null
      stage.dispose()
      current.dispose()
    }
  }, [asset, appearance.sprite, gl, scene, onReady])
  useEffect(() => {
    character?.setAppearance(appearance)
  }, [character, appearance])
  useEffect(() => {
    character?.setClip(clip)
  }, [character, clip])
  useFrame(({ clock }, delta) => {
    if (!character) return
    character.tick(Math.min(delta, 0.1))
    const blink = Math.max(
      0,
      1 - Math.abs((clock.elapsedTime % 4.8) - 4.6) / 0.12,
    )
    const face =
      expression ??
      (character.clip === 'Celebrate'
        ? 'Smile'
        : character.clip === 'Error'
          ? 'Concern'
          : character.clip === 'NeedsInput'
            ? 'Surprise'
            : 'Neutral')
    for (const name of ['Blink', 'Smile', 'Surprise', 'Concern'] as const)
      character.setExpression(
        name,
        face === name ? 1 : name === 'Blink' ? blink : 0,
      )
  })
  return character ? (
    <primitive object={character.scene} dispose={null} />
  ) : null
}
export default function SpriteScene(props: Props) {
  const camera = spriteCatalog[props.appearance.sprite].camera
  return (
    <Canvas
      key={props.appearance.sprite}
      camera={{
        position: camera.position as [number, number, number],
        fov: camera.fov,
        near: 0.1,
        far: 50,
      }}
      onCreated={(state) =>
        state.camera.lookAt(
          camera.target[0],
          camera.target[1],
          camera.target[2],
        )
      }
      className="sprite-canvas"
      dpr={[1, 1.5]}
      gl={{ alpha: true, antialias: true, powerPreference: 'low-power' }}
    >
      <Suspense fallback={null}>
        <Character {...props} />
      </Suspense>
      {props.interactive && (
        <OrbitControls
          target={
            spriteCatalog[props.appearance.sprite].camera.target as [
              number,
              number,
              number,
            ]
          }
          enablePan={false}
          minDistance={5}
          maxDistance={14}
          minPolarAngle={0.3}
          maxPolarAngle={2.4}
        />
      )}
    </Canvas>
  )
}
