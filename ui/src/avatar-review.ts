// The development page for the sprite models (assets/avatars/README.md).
// It draws Brownie and Pebble with the camera, the lighting and the
// animation controller the app itself uses, beside the illustration each
// model comes from, so a model change is judged against its reference.
// `vite build` takes index.html alone, so this page is a development
// tool and never part of the bundle the daemon serves.

import { WebGLRenderer } from 'three'

import reference from '../../assets/avatars/pixie/source/reference.png?url'
import { spriteCatalog } from './avatars/catalog'
import type { SpriteClip, SpriteExpression } from './avatars/motion'
import { loadSprite, spriteStage } from './avatars/rendering'
import { createSprite } from './avatars/sprite'

const families = ['brownie', 'pebble']
const clips: SpriteClip[] = [
  'Idle',
  'Working',
  'Waiting',
  'NeedsInput',
  'Celebrate',
  'Error',
]
// The morph targets the models carry. `Neutral` is the absence of all of
// them, so it is a choice in the list and not a target to drive.
const morphs: SpriteExpression[] = ['Blink', 'Smile', 'Surprise', 'Concern']
const expressions: SpriteExpression[] = ['Neutral', ...morphs]

interface Character {
  renderer: WebGLRenderer
  stage: ReturnType<typeof spriteStage>
  avatar: ReturnType<typeof createSprite>
}

/** The page holds the empty control; the names come from the types. */
function picker(id: string, names: readonly string[]): HTMLSelectElement {
  const node = document.querySelector<HTMLSelectElement>(`#${id}`)!
  node.append(...names.map((name) => new Option(name)))
  return node
}

const gallery = document.querySelector('#characters')!
const characters: Character[] = []
for (const family of families) {
  const definition = spriteCatalog[family]
  const article = document.createElement('article')
  const heading = document.createElement('h2')
  heading.textContent = definition.label
  article.append(heading)
  gallery.append(article)

  const renderer = new WebGLRenderer({ alpha: true, antialias: true })
  renderer.setSize(440, 440)
  renderer.setPixelRatio(Math.min(devicePixelRatio, 2))
  article.append(renderer.domElement)

  const illustration = document.createElement('div')
  illustration.className = `reference ${family}`
  illustration.style.backgroundImage = `url(${reference})`
  illustration.setAttribute('aria-label', `${family} illustration`)
  article.append(illustration)

  const stage = spriteStage(renderer, definition)
  const avatar = createSprite(await loadSprite(family), definition)
  stage.scene.add(avatar.scene)
  characters.push({ renderer, stage, avatar })
}

document.querySelector('#status')!.textContent =
  'Both models loaded. Six clips and four facial controls are available.'

const clipPicker = picker('clip', clips)
clipPicker.addEventListener('change', () => {
  for (const { avatar } of characters)
    avatar.setClip(clipPicker.value as SpriteClip)
})

const expressionPicker = picker('expression', expressions)
expressionPicker.addEventListener('change', () => {
  for (const { avatar } of characters)
    for (const name of morphs)
      avatar.setExpression(name, name === expressionPicker.value ? 1 : 0)
})

const pause = document.querySelector<HTMLButtonElement>('#pause')!
let playing = true
pause.addEventListener('click', () => {
  playing = !playing
  pause.textContent = playing ? 'Pause' : 'Play'
})

let last = performance.now()
function frame(now: number) {
  const delta = Math.min((now - last) / 1000, 0.1)
  last = now
  for (const { avatar, renderer, stage } of characters) {
    if (playing) avatar.tick(delta)
    renderer.render(stage.scene, stage.camera)
  }
  requestAnimationFrame(frame)
}
requestAnimationFrame(frame)
