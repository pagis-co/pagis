import {
  AnimationMixer,
  LoopOnce,
  LoopRepeat,
  Material,
  Mesh,
  MeshStandardMaterial,
  SkinnedMesh,
} from 'three'
import { clone } from 'three/addons/utils/SkeletonUtils.js'
import type { GLTF } from 'three/addons/loaders/GLTFLoader.js'

import type { SpriteDefinition, SpriteAppearance } from './catalog'
import type { SpriteClip, SpriteExpression } from './motion'

/** Each avatar owns its materials and bones. Mesh geometry stays shared. */
export function createSprite(
  asset: Pick<GLTF, 'scene' | 'animations'>,
  manifest: SpriteDefinition,
) {
  const scene = clone(asset.scene)
  const materials = new Map<Material, Material>()
  scene.traverse((object) => {
    if (!(object instanceof Mesh)) return
    const own = (source: Material) => {
      if (!materials.has(source)) materials.set(source, source.clone())
      return materials.get(source)!
    }
    object.material = Array.isArray(object.material)
      ? object.material.map(own)
      : own(object.material)
  })

  function setAppearance(appearance: SpriteAppearance) {
    const preset = manifest.presets[appearance.preset]
    const colors: Record<string, string> = { ...preset.materials }
    for (const [slot, color] of Object.entries(appearance.colors)) {
      for (const material of manifest.colors[slot].materials)
        colors[material] = color
    }
    for (const mat of materials.values()) {
      if (mat instanceof MeshStandardMaterial && colors[mat.name]) {
        mat.color.set(colors[mat.name])
      }
    }
    for (const name of Object.keys(manifest.accessoryLabels)) {
      scene.traverse((object) => {
        if (object.userData.accessory === name) {
          object.visible =
            appearance.accessories[name] ?? preset.accessories[name]
        }
      })
    }
  }

  const mixer = new AnimationMixer(scene)
  const actions = new Map(
    asset.animations.map((clip) => [clip.name, mixer.clipAction(clip)]),
  )
  let current: SpriteClip = 'Idle'
  let active = actions.get(current)!
  const retire = new Map<typeof active, number>()
  active.play()

  function setClip(name: SpriteClip) {
    if (name === current && active.isRunning()) return
    const next = actions.get(name)
    if (!next) throw new Error(`Sprite has no ${name} animation`)
    const prior = active
    current = name
    active = next
    retire.delete(next)
    next.reset().setEffectiveTimeScale(1).setEffectiveWeight(1)
    next.setLoop(manifest.clips[name].loop ? LoopRepeat : LoopOnce, Infinity)
    next.clampWhenFinished = !manifest.clips[name].loop
    next.play()
    if (prior !== next) {
      next.crossFadeFrom(prior, 0.25, false)
      retire.set(prior, mixer.time + 0.25)
    }
  }

  const finished = (event: { action: typeof active }) => {
    if (event.action === active) setClip('Idle')
  }
  mixer.addEventListener('finished', finished)

  setAppearance({
    sprite: manifest.family,
    preset: manifest.defaultPreset,
    colors: {},
    accessories: {},
  })
  return {
    scene,
    get clip() {
      return current
    },
    setAppearance,
    setClip,
    setExpression(name: SpriteExpression, value: number) {
      scene.traverse((object) => {
        if (!(object instanceof Mesh)) return
        const index = object.morphTargetDictionary?.[name]
        if (index !== undefined && object.morphTargetInfluences) {
          object.morphTargetInfluences[index] = Math.max(0, Math.min(1, value))
        }
      })
    },
    tick(seconds: number) {
      mixer.update(seconds)
      for (const [action, time] of retire) {
        if (mixer.time >= time) {
          action.stop()
          retire.delete(action)
        }
      }
    },
    dispose() {
      mixer.removeEventListener('finished', finished)
      mixer.stopAllAction()
      mixer.uncacheRoot(scene)
      const skeletons = new Set<SkinnedMesh['skeleton']>()
      scene.traverse((object) => {
        if (object instanceof SkinnedMesh) skeletons.add(object.skeleton)
      })
      for (const skeleton of skeletons) skeleton.dispose()
      for (const mat of materials.values()) mat.dispose()
    },
  }
}
