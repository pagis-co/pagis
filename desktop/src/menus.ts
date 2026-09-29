import * as path from 'node:path'

import { Menu, type MenuItemConstructorOptions, Tray, app, shell } from 'electron'

import type { NewVersion } from './updateCheck'

export interface MenuActions {
  open(): void
  /** Open the Administration Interface on its own port. */
  openAdministration(): void
  quit(): void
  openAtLogin(open: boolean): void
  isOpenAtLogin(): boolean
  newVersion(): NewVersion | null
}

/**
 * The icon of the tray item (scripts/draw-tray-icons.mjs draws them).
 * macOS takes a 16 point template image, which it draws in the colour of
 * the menu bar, and loads `trayTemplate@2x.png` beside it for a Retina
 * screen. Linux shows the mark in its colors, and the status area scales
 * the image to its own size.
 */
export function trayIcon(platform: string = process.platform): string {
  return path.join(__dirname, '..', 'static', platform === 'darwin' ? 'trayTemplate.png' : 'tray.png')
}

/**
 * The tray item (ADR-0025), in the macOS menu bar and in the Linux
 * status area: open the window again after a close, hold "Open at
 * login", carry the new-version line, and quit.
 *
 * A Linux status area shows the item through StatusNotifierItem, and
 * there a click opens the menu and sends no click event, so every action
 * is a menu item. A desktop with no status area shows no item; the
 * person then opens Pagis from the application launcher, and the
 * single-instance lock shows the running window.
 */
export function createTray(actions: MenuActions): Tray {
  const tray = new Tray(trayIcon())
  tray.setToolTip('Pagis')
  renderTray(tray, actions)
  return tray
}

export function renderTray(tray: Tray, actions: MenuActions): void {
  const found = actions.newVersion()
  const items: MenuItemConstructorOptions[] = [
    { label: 'Open Pagis', click: () => actions.open() },
    { label: 'Administration', click: () => actions.openAdministration() },
    { type: 'separator' },
    {
      label: 'Open at login',
      type: 'checkbox',
      checked: actions.isOpenAtLogin(),
      click: (item) => actions.openAtLogin(item.checked),
    },
  ]
  if (found) {
    items.push(
      { type: 'separator' },
      {
        label: `Pagis ${found.version} is out`,
        click: () => void shell.openExternal(found.url),
      },
    )
  }
  items.push(
    { type: 'separator' },
    { label: 'Quit Pagis', click: () => actions.quit() },
  )
  tray.setContextMenu(Menu.buildFromTemplate(items))
}

/**
 * The native menu. The product's own menus live in the SPA. macOS has
 * an application menu with its own roles; Linux shows the menu in each
 * window, where a File menu holds Quit.
 */
export function applicationMenu(actions: MenuActions, platform: string = process.platform): Menu {
  const quit: MenuItemConstructorOptions = {
    label: 'Quit Pagis',
    accelerator: 'CmdOrCtrl+Q',
    click: () => actions.quit(),
  }
  const first: MenuItemConstructorOptions = platform === 'darwin'
    ? {
        label: app.name,
        submenu: [
          { role: 'about' },
          { type: 'separator' },
          { role: 'hide' },
          { role: 'hideOthers' },
          { role: 'unhide' },
          { type: 'separator' },
          quit,
        ],
      }
    : { label: 'File', submenu: [quit] }
  return Menu.buildFromTemplate([
    first,
    { role: 'editMenu' },
    {
      label: 'Installation',
      submenu: [
        {
          // The Administration Interface answers on its own port:
          // the people, the spend, the sessions, the resources and the
          // installation settings.
          label: 'Administration',
          click: () => actions.openAdministration(),
        },
      ],
    },
    {
      label: 'View',
      submenu: [
        { role: 'reload' },
        { role: 'toggleDevTools' },
        { type: 'separator' },
        { role: 'resetZoom' },
        { role: 'zoomIn' },
        { role: 'zoomOut' },
        { type: 'separator' },
        { role: 'togglefullscreen' },
      ],
    },
    { role: 'windowMenu' },
  ])
}
