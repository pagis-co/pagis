// The `/` command menu of the composer. The field keeps the
// focus while the menu is open, so the menu takes no focus of its own:
// the composer owns the active item and the keys, and this draws it.

import { Button } from '../../primitives'

export interface Command {
  id: string
  label: string
  description: string
  run: () => void
}

/** The commands whose label or description matches the typed query. */
export function matchCommands(commands: Command[], query: string): Command[] {
  const needle = query.trim().toLowerCase()
  if (needle === '') return commands
  return commands.filter(
    (command) =>
      command.label.toLowerCase().includes(needle) ||
      command.description.toLowerCase().includes(needle),
  )
}

export function CommandMenu({
  commands,
  activeIndex,
  onRun,
}: {
  commands: Command[]
  activeIndex: number
  onRun: (command: Command) => void
}) {
  if (commands.length === 0) return null
  return (
    <div
      className="composer-commands"
      role="menu"
      aria-label="Composer commands"
      data-testid="composer-commands"
    >
      {commands.map((command, index) => (
        <Button
          key={command.id}
          variant="ghost"
          role="menuitem"
          className="composer-command"
          data-active={index === activeIndex ? 'true' : undefined}
          title={command.description}
          // The field keeps the focus: the press must not move it.
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => onRun(command)}
        >
          <span className="composer-command-label">{command.label}</span>
          <span className="composer-command-description">{command.description}</span>
        </Button>
      ))}
    </div>
  )
}
