import catalog from '../../../assets/avatars/catalog.json'
import type { SpriteClip } from './motion'

export type SpriteAppearance =
  import('../api/schema').components['schemas']['AvatarAppearance']
export interface SpriteDefinition {
  family: string
  label: string
  model: string
  defaultPreset: string
  camera: { position: number[]; target: number[]; fov: number }
  clips: Record<SpriteClip, { seconds: number; loop: boolean }>
  expressions: string[]
  colors: Record<string, { label: string; materials: string[] }>
  accessoryLabels: Record<string, string>
  presets: Record<
    string,
    {
      label: string
      portrait: string
      materials: Record<string, string>
      accessories: Record<string, boolean>
    }
  >
}
export const spriteCatalog = catalog as Record<string, SpriteDefinition>
const assets = import.meta.glob(
  '../../../assets/avatars/*/{exports/*.glb,portraits/*.png}',
  {
    query: '?url',
    import: 'default',
    eager: true,
  },
) as Record<string, string>
export function spriteAsset(path: string): string {
  const url = assets[`../../../assets/avatars/${path}`]
  if (!url) throw new Error(`Missing sprite asset: ${path}`)
  return url
}
export function defaultAppearance(sprite = 'pixie'): SpriteAppearance {
  return {
    sprite,
    preset: spriteCatalog[sprite].defaultPreset,
    colors: {},
    accessories: {},
  }
}
export function appearanceKey(appearance: SpriteAppearance): string {
  return JSON.stringify([
    appearance.sprite,
    appearance.preset,
    Object.entries(appearance.colors).sort(),
    Object.entries(appearance.accessories).sort(),
  ])
}
