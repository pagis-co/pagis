import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { AutostartEntry, autostartEntry } from './loginItem'

const roots: string[] = []
afterEach(() => {
  while (roots.length > 0) fs.rmSync(roots.pop()!, { recursive: true, force: true })
})

function home(): string {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-autostart-'))
  roots.push(root)
  return root
}

describe('Open at login on Linux', () => {
  it('writes and removes an XDG autostart entry', () => {
    const root = home()
    const entry = new AutostartEntry('/opt/Pagis/pagis-client', { HOME: root })

    expect(entry.isOpenAtLogin()).toBe(false)
    entry.setOpenAtLogin(true)

    const file = path.join(root, '.config', 'autostart', 'pagis-client.desktop')
    expect(entry.isOpenAtLogin()).toBe(true)
    expect(fs.readFileSync(file, 'utf8')).toContain('Exec="/opt/Pagis/pagis-client"\n')

    entry.setOpenAtLogin(false)
    expect(fs.existsSync(file)).toBe(false)
    expect(() => entry.setOpenAtLogin(false)).not.toThrow()
  })

  it('starts the AppImage file, not its temporary mount, and honours XDG_CONFIG_HOME', () => {
    const root = home()
    const entry = new AutostartEntry('/tmp/.mount_PagisAbc/pagis-client', {
      HOME: '/nowhere',
      XDG_CONFIG_HOME: path.join(root, 'config'),
      APPIMAGE: '/home/me/Apps/Pagis-1.0.0-x86_64.AppImage',
    })

    entry.setOpenAtLogin(true)

    const text = fs.readFileSync(path.join(root, 'config', 'autostart', 'pagis-client.desktop'), 'utf8')
    expect(text).toContain('Exec="/home/me/Apps/Pagis-1.0.0-x86_64.AppImage"')
  })

  it('quotes an executable path as the Desktop Entry Specification says', () => {
    expect(autostartEntry('/home/me/My $Apps/"Pagis"%.AppImage'))
      .toContain('Exec="/home/me/My \\$Apps/\\"Pagis\\"%%.AppImage"')
  })
})
