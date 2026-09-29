import {
  DirectionalLight,
  HemisphereLight,
  PerspectiveCamera,
  PMREMGenerator,
  Scene,
  WebGLRenderer,
  SRGBColorSpace,
  ACESFilmicToneMapping,
} from 'three'
import { RoomEnvironment } from 'three/addons/environments/RoomEnvironment.js'
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js'
import {
  appearanceKey,
  spriteAsset,
  spriteCatalog,
  type SpriteAppearance,
  type SpriteDefinition,
} from './catalog'
import { createSprite } from './sprite'

const models = new Map<string, ReturnType<GLTFLoader['loadAsync']>>()
export function loadSprite(id: string) {
  const url = spriteAsset(spriteCatalog[id].model)
  if (!models.has(url)) {
    const task = new GLTFLoader().loadAsync(url).catch((error: unknown) => {
      models.delete(url)
      throw error
    })
    models.set(url, task)
  }
  return models.get(url)!
}

/** The still image and the animated view use the same camera and light. */
export function spriteStage(
  renderer: WebGLRenderer,
  definition: SpriteDefinition,
) {
  const scene = new Scene()
  const camera = new PerspectiveCamera(definition.camera.fov, 1, 0.1, 50)
  camera.position.fromArray(definition.camera.position)
  camera.lookAt(
    definition.camera.target[0],
    definition.camera.target[1],
    definition.camera.target[2],
  )
  scene.add(new HemisphereLight('#fff1d9', '#ead9b8', 1.6))
  for (const [position, intensity] of [
    [[-3, 6, 4], 2.8],
    [[4, 2, 5], 1.6],
    [[1, 4, -3], 2],
  ] as const) {
    const light = new DirectionalLight('#fff0d8', intensity)
    light.position.fromArray(position)
    scene.add(light)
  }
  const room = new RoomEnvironment()
  const generator = new PMREMGenerator(renderer)
  const environment = generator.fromScene(room)
  room.dispose()
  generator.dispose()
  scene.environment = environment.texture
  scene.environmentIntensity = 0.5
  renderer.toneMapping = ACESFilmicToneMapping
  renderer.outputColorSpace = SRGBColorSpace
  return { scene, camera, dispose: () => environment.dispose() }
}

let renderer: WebGLRenderer | undefined
let queue = Promise.resolve()
const portraits = new Map<string, Promise<string>>()

/** One shared renderer makes still images. Lists do not keep WebGL canvases. */
export function spritePortrait(appearance: SpriteAppearance): Promise<string> {
  const key = appearanceKey(appearance)
  const existing = portraits.get(key)
  if (existing) return existing
  const result = queue.then(async () => {
    const asset = await loadSprite(appearance.sprite)
    renderer ??= new WebGLRenderer({
      alpha: true,
      antialias: true,
      powerPreference: 'low-power',
    })
    renderer.setSize(384, 384, false)
    const stage = spriteStage(renderer, spriteCatalog[appearance.sprite])
    const character = createSprite(asset, spriteCatalog[appearance.sprite])
    try {
      character.setAppearance(appearance)
      character.tick(0)
      stage.scene.add(character.scene)
      renderer.render(stage.scene, stage.camera)
      return renderer.domElement.toDataURL('image/png')
    } finally {
      character.dispose()
      stage.dispose()
    }
  })
  queue = result.then(
    () => undefined,
    () => {
      portraits.delete(key)
    },
  )
  portraits.set(key, result)
  if (portraits.size > 64) portraits.delete(portraits.keys().next().value!)
  return result
}
