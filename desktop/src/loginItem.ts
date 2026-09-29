import * as fs from 'node:fs'
import * as path from 'node:path'

/** "Open at login" for the tray menu. */
export interface LoginItem {
  isOpenAtLogin(): boolean
  setOpenAtLogin(open: boolean): void
}

/**
 * Linux has no login item API, so "Open at login" is an XDG autostart
 * entry, as Slack and Signal write one: `pagis-client.desktop` in
 * `$XDG_CONFIG_HOME/autostart`, or `~/.config/autostart`. The entry
 * starts the AppImage file itself when the client runs from one, because
 * the executable path of an AppImage is a mount that exists only while it
 * runs.
 */
export class AutostartEntry implements LoginItem {
  constructor(
    private readonly execPath: string = process.execPath,
    private readonly env: NodeJS.ProcessEnv = process.env,
  ) {}

  get file(): string {
    const config = this.env.XDG_CONFIG_HOME || (this.env.HOME ? path.join(this.env.HOME, '.config') : null)
    if (!config) throw new Error('HOME is not set, so Pagis cannot find the autostart directory')
    return path.join(config, 'autostart', 'pagis-client.desktop')
  }

  isOpenAtLogin(): boolean {
    return fs.existsSync(this.file)
  }

  setOpenAtLogin(open: boolean): void {
    if (!open) {
      fs.rmSync(this.file, { force: true })
      return
    }
    fs.mkdirSync(path.dirname(this.file), { recursive: true })
    fs.writeFileSync(this.file, autostartEntry(this.env.APPIMAGE || this.execPath))
  }
}

/** A desktop entry that starts `executable`. */
export function autostartEntry(executable: string): string {
  return [
    '[Desktop Entry]',
    'Type=Application',
    'Name=Pagis',
    `Exec=${quoteExec(executable)}`,
    'X-GNOME-Autostart-enabled=true',
    '',
  ].join('\n')
}

/**
 * Quote one argument of an `Exec` key as the Desktop Entry
 * Specification says: in double quotes, with `"`, `` ` ``, `$` and `\`
 * escaped by a backslash. A literal `%` is doubled, because `%` starts a
 * field code.
 */
function quoteExec(argument: string): string {
  return `"${argument.replace(/(["`$\\])/g, '\\$1').replace(/%/g, '%%')}"`
}
