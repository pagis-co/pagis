import * as path from 'node:path'
import { BrowserWindow, session, type IpcMainInvokeEvent, type Session } from 'electron'
import { BrowserSignIns } from './browserSignIn'
import { ExitTraffic } from './exit'
import { EXIT_CAPABILITY, SHELL_CAPABILITY } from './host'
import { opensInSystemBrowser } from './origin'
import { type ServerConnection, ServerConnections } from './serverConnection'
import { hostLinkFor, type HostLinks } from './serverHost'
import { openServerPage, watchServerSignIn } from './serverSignIn'
import { installBluetoothRefusal, installPermissionHandlers } from './webPermissions'
import { applyProductWebRtcPolicy, productWindowChrome } from './windowChrome'
import { handleWindowClose } from './windowClose'
import { installNavigationRule } from './windowNavigation'

interface ServerWindow {
  connection: ServerConnection
  session: Session
  window: BrowserWindow | null
  watch: (() => void) | null
  host: HostLinks | null
  traffic: ExitTraffic
  verifier?: { secret: string; value: string; expires: number }
  removing: boolean
}

/** Each connected server owns its window, partition, sign-in watch and Host link. */
export class ServerWindows {
  private readonly held = new Map<string, ServerWindow>()
  private readonly signIns = new BrowserSignIns()
  currentId: string | null = null

  constructor(private readonly connections: ServerConnections, private readonly actions: {
    changed(): void
    openExternal(url: string): void
    quitting(): boolean
  }) {}

  visible(): BrowserWindow | undefined { return [...this.held.values()].map(({ window }) => window).find((window) => window?.isVisible()) ?? undefined }
  codingSessions(): number { return [...this.held.values()].reduce((sum, server) => sum + (server.host?.codingSessions() ?? 0), 0) }
  exitTraffic(): { id: string; label: string; traffic: ExitTraffic }[] {
    return [...this.held.values()].map(({ connection, traffic }) => ({ id: connection.id, label: connection.label, traffic }))
  }
  jar(id: string): Session['cookies'] | undefined { return this.held.get(id)?.session.cookies }

  /** Focus a window without reloading the Product App. */
  show(id: string): boolean {
    const window = this.held.get(id)?.window
    if (!window) return false
    window.show(); window.focus()
    return true
  }

  async open(connection: ServerConnection, url: string, verifier?: { secret: string; value: string }): Promise<void> {
    if (new URL(url).origin !== new URL(connection.origin).origin) throw new Error('the page is not on this server')
    let server = this.held.get(connection.id)
    if (!server) {
      server = {
        connection, session: session.fromPartition(`persist:server-${connection.id}`), window: null,
        watch: null, host: null, traffic: new ExitTraffic(() => this.actions.changed()), removing: false,
      }
      this.held.set(connection.id, server)
      const owned = server
      installPermissionHandlers(server.session, () => owned.window ? { contents: owned.window.webContents, origin: new URL(owned.connection.origin).origin } : null, process.platform)
      server.session.webRequest.onBeforeRedirect((details) => {
        if (owned.removing || details.resourceType !== 'mainFrame') return
        const moved = this.connections.followRedirect(owned.connection.id, details.statusCode, details.url, details.redirectURL)
        if (!moved) return
        Object.assign(owned.connection, moved)
        delete owned.verifier
        this.signIns.remove(moved.id)
        owned.window?.setTitle(`Pagis — ${moved.label}`)
        owned.watch?.(); owned.watch = null
        const host = owned.host
        owned.host = null
        void host?.stop()
        this.watch(owned)
        this.actions.changed()
      })
    }
    if (server.removing) throw new Error('this server is being removed')
    if (!server.window) {
      const owned = server
      const window = new BrowserWindow({
        width: 1180, height: 820, title: `Pagis — ${connection.label}`, show: false, autoHideMenuBar: true,
        ...productWindowChrome(process.platform),
        webPreferences: { partition: `persist:server-${connection.id}`, preload: path.join(__dirname, 'productPreload.js'), nodeIntegration: false, contextIsolation: true, sandbox: true },
      })
      server.window = window
      applyProductWebRtcPolicy(window.webContents)
      window.on('focus', () => { this.currentId = connection.id; this.actions.changed() })
      window.on('close', (event) => handleWindowClose(event, this.actions.quitting() || owned.removing, window))
      window.on('closed', () => { owned.window = null; delete owned.verifier })
      installNavigationRule(window.webContents, () => ({ window: 'product', origin: new URL(owned.connection.origin).origin }), this.actions.openExternal)
      installBluetoothRefusal(window.webContents)
      window.webContents.setWindowOpenHandler(({ url: target }) => {
        if (opensInSystemBrowser(target, owned.connection.origin)) this.actions.openExternal(target)
        return { action: 'deny' }
      })
    }
    this.watch(server)
    server.verifier = verifier ? { ...verifier, expires: Date.now() + 300_000 } : undefined
    await openServerPage(server.window, url)
    this.show(connection.id)
  }

  private watch(server: ServerWindow): void {
    if (server.watch) return
    server.watch = watchServerSignIn(server.connection.origin, server.session.cookies, () => {
      if (server.removing || server.host) return
      server.host = hostLinkFor({ url: server.connection.origin, jar: server.session.cookies, credential: () => null, capabilities: [SHELL_CAPABILITY, EXIT_CAPABILITY], traffic: server.traffic })
      server.host.start()
    })
  }

  beginSignIn(server: ServerConnection): string { return this.signIns.begin(server) }

  async acceptSignIn(link: string): Promise<boolean> {
    const accepted = this.signIns.accept(link)
    if (!accepted || !this.connections.list().some(({ id }) => id === accepted.server.id)) return false
    // Create the window before its first load, so the page can ask for the verifier at once.
    try {
      await this.open(accepted.server, accepted.url, { secret: accepted.secret, value: accepted.verifier })
    } catch (error) {
      const server = this.held.get(accepted.server.id)
      if (server) delete server.verifier
      throw error
    }
    return true
  }

  /** Only the sign-in main frame of the window that owns the secret receives it. */
  takeVerifier(event: Pick<IpcMainInvokeEvent, 'sender' | 'senderFrame'>, secret: unknown): string | undefined {
    if (typeof secret !== 'string') return undefined
    for (const server of this.held.values()) {
      const contents = server.window?.webContents
      if (server.removing || !contents || event.sender !== contents || event.senderFrame !== contents.mainFrame) continue
      const url = new URL(contents.mainFrame.url)
      const verifier = server.verifier
      if (url.origin !== new URL(server.connection.origin).origin || url.pathname !== '/sign-in' ||
        !verifier || verifier.secret !== secret || verifier.expires <= Date.now()) return undefined
      delete server.verifier
      return verifier.value
    }
    return undefined
  }

  async remove(id: string): Promise<void> {
    this.signIns.remove(id)
    const server = this.held.get(id)
    if (server) {
      server.removing = true
      server.watch?.(); server.watch = null
      delete server.verifier
      server.window?.destroy()
      await server.host?.stop()
    }
    const partition = server?.session ?? session.fromPartition(`persist:server-${id}`)
    await partition.clearStorageData()
    await partition.clearCache()
    this.connections.remove(id)
    this.held.delete(id)
    if (this.currentId === id) this.currentId = this.connections.list()[0]?.id ?? null
    this.actions.changed()
  }

  async stop(): Promise<void> {
    for (const server of this.held.values()) { server.removing = true; server.watch?.(); delete server.verifier }
    await Promise.all([...this.held.values()].map(async (server) => { await server.host?.stop() }))
  }
}
