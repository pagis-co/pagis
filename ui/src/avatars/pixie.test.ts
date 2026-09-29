import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js'
import { DataTexture, Mesh, MeshStandardMaterial, RGBAFormat } from 'three'
import { decode } from 'fast-png'

import { createSprite } from './sprite'
import { spriteCatalog, defaultAppearance } from './catalog'

async function asset(path = 'pixie/exports/pixie.glb') {
  const data = readFileSync(
    resolve(
      dirname(fileURLToPath(import.meta.url)),
      `../../../assets/avatars/${path}`,
    ),
  )
  const buffer = new ArrayBuffer(data.byteLength)
  new Uint8Array(buffer).set(data)
  const loader = new GLTFLoader()
  // jsdom cannot decode browser images. Decode the embedded PNG to real pixels
  // at the texture boundary; the model and controller use the real GLTFLoader.
  loader.register((parser) => ({
    name: 'TestEmbeddedPng',
    async loadTexture(index: number) {
      const source = parser.json.images[parser.json.textures[index].source]
      const bytes = await parser.getDependency('bufferView', source.bufferView)
      const png = decode(new Uint8Array(bytes), { checkCrc: true })
      expect(png.channels).toBe(4)
      expect(png.depth).toBe(8)
      const texture = new DataTexture(
        png.data,
        png.width,
        png.height,
        RGBAFormat,
      )
      texture.flipY = false
      texture.needsUpdate = true
      return texture
    },
  }))
  return loader.parseAsync(buffer, '')
}

describe('Pixie GLB in the browser loader', () => {
  it('loads independent characters whose colors and accessories can change', async () => {
    const gltf = await asset()
    const mint = createSprite(gltf, spriteCatalog.pixie)
    const lavender = createSprite(gltf, spriteCatalog.pixie)
    lavender.setAppearance({
      ...defaultAppearance(),
      preset: 'lavender',
      accessories: { scarf: true },
    })
    expect(mint.scene.getObjectByName('Accessory_Glasses')?.visible).toBe(false)
    expect(lavender.scene.getObjectByName('Accessory_Glasses')?.visible).toBe(
      true,
    )
    expect(lavender.scene.getObjectByName('Accessory_Scarf')?.visible).toBe(
      true,
    )
    const body = (pixie: typeof mint) => {
      let result: MeshStandardMaterial | undefined
      pixie.scene.traverse((object) => {
        if (!(object instanceof Mesh)) return
        for (const mat of Array.isArray(object.material)
          ? object.material
          : [object.material]) {
          if (mat.name === 'Body') result = mat
        }
      })
      return result!
    }
    expect(body(mint).color.getHexString()).toBe('adca64')
    expect(body(lavender).color.getHexString()).toBe('b9a5ce')
    expect(body(mint)).not.toBe(body(lavender))
    lavender.setAppearance({
      ...defaultAppearance(),
      colors: { body: '#123456' },
      accessories: { satchel: false },
    })
    expect(body(lavender).color.getHexString()).toBe('123456')
    expect(lavender.scene.getObjectByName('Accessory_Satchel')?.visible).toBe(
      false,
    )
    lavender.setAppearance(defaultAppearance())
    expect(body(lavender).color.getHexString()).toBe('adca64')
    expect(body(mint).normalMap).toMatchObject({
      image: { width: 512, height: 512 },
    })
    expect(body(mint).normalMap).toBe(body(lavender).normalMap)
    mint.dispose()
    lavender.dispose()
  })

  it('plays all six clips and a new state can interrupt a one-time action', async () => {
    const pixie = createSprite(await asset(), spriteCatalog.pixie)
    const head = pixie.scene.getObjectByName('Head')!
    for (const clip of [
      'Idle',
      'Working',
      'Waiting',
      'NeedsInput',
      'Celebrate',
      'Error',
    ] as const) {
      pixie.setClip(clip)
      pixie.tick(0)
      const before = head.quaternion.clone()
      pixie.tick(0.5)
      expect(head.quaternion.equals(before), clip).toBe(false)
    }
    pixie.setClip('Celebrate')
    pixie.tick(2.5)
    expect(pixie.clip).toBe('Idle')
    pixie.setClip('Celebrate')
    pixie.tick(0.2)
    pixie.setClip('Working')
    pixie.tick(3)
    expect(pixie.clip).toBe('Working')
    pixie.dispose()
  })

  it('sets facial expressions without changing another avatar', async () => {
    const gltf = await asset()
    const first = createSprite(gltf, spriteCatalog.pixie)
    const second = createSprite(gltf, spriteCatalog.pixie)
    first.setExpression('Blink', 1)
    first.setExpression('Surprise', 0.8)
    let blinks = 0
    first.scene.traverse((object) => {
      if (!(object instanceof Mesh) || !object.morphTargetDictionary) return
      const index = object.morphTargetDictionary.Blink
      if (index !== undefined) {
        expect(object.morphTargetInfluences?.[index]).toBe(1)
        blinks++
      }
    })
    expect(blinks).toBeGreaterThan(0)
    second.scene.traverse((object) => {
      if (object instanceof Mesh && object.morphTargetInfluences) {
        expect(object.morphTargetInfluences.every((value) => value === 0)).toBe(
          true,
        )
      }
    })
    first.dispose()
    second.dispose()
  })
})

describe('the sprite catalog GLB contract', () => {
  it.each(Object.entries(spriteCatalog))(
    '%s exposes every configured control',
    async (id, definition) => {
      const gltf = await asset(definition.model)
      const materials = new Set<string>()
      const accessories = new Set<string>()
      const expressions = new Set<string>()
      gltf.scene.traverse((object) => {
        if (object.userData.accessory)
          accessories.add(object.userData.accessory)
        if (!(object instanceof Mesh)) return
        for (const material of Array.isArray(object.material)
          ? object.material
          : [object.material])
          materials.add(material.name)
        for (const name of Object.keys(object.morphTargetDictionary ?? {}))
          expressions.add(name)
      })
      expect(definition.family).toBe(id)
      expect(definition.presets[definition.defaultPreset]).toBeDefined()
      for (const name of [
        'Idle',
        'Working',
        'Waiting',
        'NeedsInput',
        'Celebrate',
        'Error',
      ]) {
        const clip = gltf.animations.find((clip) => clip.name === name)
        expect(clip, name).toBeDefined()
        expect(clip!.duration).toBeCloseTo(
          definition.clips[name as keyof typeof definition.clips].seconds,
          1,
        )
      }
      for (const name of ['Blink', 'Smile', 'Surprise', 'Concern'])
        expect(expressions.has(name), name).toBe(true)
      for (const name of Object.keys(definition.accessoryLabels))
        expect(accessories.has(name), name).toBe(true)
      for (const preset of Object.values(definition.presets)) {
        for (const slot of Object.values(definition.colors))
          for (const name of slot.materials) {
            expect(materials.has(name), name).toBe(true)
            expect(preset.materials[name], name).toMatch(/^#[0-9a-f]{6}$/i)
          }
        for (const name of Object.keys(definition.accessoryLabels))
          expect(typeof preset.accessories[name], name).toBe('boolean')
      }
    },
  )
})
