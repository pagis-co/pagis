import { avatarInitial } from './avatar'
import './initial-tile.css'

export function InitialTile({ name }: { name: string }) {
  return <span className="ui-initial-tile" aria-hidden>{avatarInitial(name)}</span>
}
