import * as fs from 'node:fs'
import * as path from 'node:path'
import * as os from 'node:os'
import { pathToFileURL } from 'node:url'

import { BrowserWindow, Menu, type Tray, app, dialog, ipcMain, session, shell } from 'electron'

import { ClientController } from './clientController'
import { DaemonSupervisor, redact, type DaemonState } from './daemon'
import { openSignedIn, openSignedInAt } from './clientSession'
import { endOnQuit } from './appQuit'
import {
  administrationUrl,
  daemonUrl,
  dataDirectory,
  readAdministrationPort,
  readClientCredential,
  readPort,
} from './dataDirectory'
import { HostLink } from './host'
import { opensInSystemBrowser } from './origin'
import { hostLinkFor } from './serverHost'
import { AutostartEntry, type LoginItem } from './loginItem'
import { applicationMenu, createTray, renderTray } from './menus'
import { PidFile } from './pidFile'
import { quitQuestion } from './quitQuestion'
import { executableInUse } from './runtimeUse'
import { installationError, RuntimeInstaller } from './runtimeInstaller'
import {
  NoRuntimeLockError,
  readRuntimeLock,
  runtimeLockFile,
  thisPlatform,
  type RuntimeLock,
} from './runtimeLock'
import { RuntimeState } from './runtimeState'
import { probeRuntimeIdentity } from './runtimeIdentity'
import { recoveryView, sameProductOrigin } from './recoveryView'
import { type SetupState, setupFailureState } from './setupState'
import { isTrustedSetupRequest } from './setupTrust'
import { isServerRequest } from './setupCoordinator'
import { assertServerIsReady, connectToServer } from './serverOnboarding'
import { watchServerSignIn } from './serverSignIn'
import { ServerConnections, type ServerConnection } from './serverConnection'
import { reportUncaughtExceptions } from './uncaughtFailure'
import { type NewVersion, checkForNewVersion } from './updateCheck'
import { installBluetoothRefusal, installPermissionHandlers, type ProductWindow } from './webPermissions'
import {
  applyProductWebRtcPolicy,
  productWindowChrome,
  setupWindowOptions,
  statusWindowOptions,
} from './windowChrome'
import { handleWindowClose } from './windowClose'
import { installNavigationRule } from './windowNavigation'

/** The Multi-User Mode switch in the Settings view of the Administration
 *  Interface. Setup for several People opens it after the product. */
const MULTI_USER_SWITCH = '/settings#multi-user'

const SMOKE = process.argv.includes('--smoke')
const SMOKE_DEADLINE_MS = 60000
let smokeRoot: string | null = null

class Shell {
  private readonly home = dataDirectory()
  private readonly runtimeRoot = path.join(app.getPath('userData'), 'runtime')
  private readonly runtimeState = new RuntimeState(this.runtimeRoot)
  // The server this client connects to, when it did not start one.
  // It is null for the install-and-supervise path.
  private readonly connections = new ServerConnections(app.getPath('userData'))
  private connection: ServerConnection | null = null
  // The watch for the Person's sign-in on the page of that server.
  private signInWatch: (() => void) | null = null
  private mainWindow: BrowserWindow | null = null
  private mainOrigin: string | null = null
  private administrationWindow: BrowserWindow | null = null
  private administrationOrigin: string | null = null
  private setupWindow: BrowserWindow | null = null
  private statusWindow: BrowserWindow | null = null
  private tray: Tray | null = null
  private update: NewVersion | null = null
  private setupState: SetupState = { kind: 'ready' }
  private lock: RuntimeLock | null = null
  private readonly installer = new RuntimeInstaller(this.runtimeRoot)
  // This machine as a Host. A host action runs here, through the
  // client, and never in the daemon.
  private hostLink: HostLink | null = null
  private quitting = false
  private readonly controller = new ClientController({
    install: (options) => {
      this.setSetupState({ kind: 'installing', detail: 'Downloading and checking the Pagis server…' })
      return this.installer.install(this.runtimeLock(), options)
    },
    beginLaunch: () => this.runtimeState.beginLaunch(this.runtimeLock().release),
    connect: (request, signal) => this.connect(request, signal),
    activate: () => this.activateOwnRuntime(),
    openProduct: (url) => this.finishHandoff(url),
    openMultiUserSwitch: () => this.openAdministration(MULTI_USER_SWITCH),
    createSupervisor: (binary, beforeSpawn) => this.createSupervisor(binary, beforeSpawn),
    assertNoExternalRuntime: () => this.assertNoExternalRuntime(),
    onDaemonState: (state) => this.renderSupervisorState(state),
  })

  async start(): Promise<void> {
    // The handlers go on the session before the first window opens,
    // because Electron grants each permission that no handler decides.
    // All windows use this one session.
    installPermissionHandlers(session.defaultSession, () => this.productWindow(), process.platform)
    this.handleRequests()
    // electron-builder gives the packaged app the icon in build/. A run
    // from source is the Electron bundle, so it sets the same icon for
    // the Dock and the message boxes.
    if (!app.isPackaged) app.dock?.setIcon(path.join(__dirname, '..', 'build', 'icon.png'))
    if (!SMOKE) {
      Menu.setApplicationMenu(applicationMenu(this.menuActions()))
      this.tray = createTray(this.menuActions())
    }
    let connection: ServerConnection | null
    try {
      connection = this.connections.read()
    } catch (error) {
      await this.openSetup()
      this.setSetupState(this.failureState(error))
      if (!SMOKE) void this.checkVersion()
      return
    }
    if (connection) {
      await this.openConnected(connection)
      if (!SMOKE) void this.checkVersion()
      return
    }
    let release: string | null
    try {
      release = this.runtimeState.releaseToStart()
    } catch (error) {
      await this.openSetup()
      this.setSetupState(this.failureState(error))
      if (!SMOKE) void this.checkVersion()
      return
    }
    if (release === app.getVersion()) {
      try {
        await this.controller.resume()
      } catch (error) {
        if (this.controller.state?.kind !== 'taken-port') {
          await this.openSetup()
          this.setSetupState(this.failureState(error))
        }
      }
    } else {
      await this.openSetup()
    }
    if (SMOKE) {
      console.log('pagis smoke: setup opened without a bundled server')
      if (smokeRoot) fs.rmSync(smokeRoot, { recursive: true, force: true })
      app.exit(0)
      return
    }
    void this.checkVersion()
  }

  /** Stop the work of the client before the process ends (`endOnQuit`). */
  async stop(): Promise<void> {
    this.quitting = true
    // The machine is absent the moment the client goes, so the socket
    // closes before anything else does.
    this.signInWatch?.()
    this.signInWatch = null
    this.hostLink?.stop()
    this.hostLink = null
    await this.controller.cancel()
  }

  /** Quit from the setup page, the app menu or the tray. It asks first
   *  only while an installation or a start-up is in progress. */
  async requestQuit(): Promise<void> {
    const question = quitQuestion(this.controller.inProgress)
    if (question === null) {
      app.quit()
      return
    }
    // The question is a sheet on a visible window, the one that "Open"
    // shows when none is visible. On macOS a message box with no window
    // runs a modal loop that holds every other quit, SIGTERM too, until
    // the Person answers it.
    const visible = (): BrowserWindow | null =>
      [this.setupWindow, this.statusWindow, this.mainWindow].find((window) => window?.isVisible()) ?? null
    if (visible() === null) this.show()
    const window = visible()
    const answer = window === null
      ? await dialog.showMessageBox(question)
      : await dialog.showMessageBox(window, question)
    if (answer.response === 0) app.quit()
  }

  /** Put a failure that no code caught on the setup page. */
  showUncaught(error: Error): void {
    void this.openSetup()
      .then(() => this.setSetupState(this.failureState(error)))
      .catch((failure: unknown) => console.error(`pagis: the setup page did not open: ${String(failure)}`))
  }

  get isQuitting(): boolean { return this.quitting }

  show(): void {
    if (this.setupWindow) {
      this.setupWindow.show()
      this.setupWindow.focus()
      return
    }
    // A connect-only client supervises no process, so the window is the
    // whole of what "open" means to it.
    if (this.connection) {
      if (this.mainWindow) {
        this.mainWindow.show()
        this.mainWindow.focus()
      } else {
        void this.openConnected(this.connection)
      }
      return
    }
    const state = this.controller.state
    if (state?.kind === 'running') {
      if (this.mainWindow) {
        this.mainWindow.show()
        this.mainWindow.focus()
      } else {
        void this.openMain(state.url).catch((error: unknown) => this.reportHandoffFailure(error))
      }
      return
    }
    const recovery = this.setupWindow ?? this.statusWindow
    if (recovery) {
      recovery.show()
      recovery.focus()
    } else if (state && state.kind !== 'starting') this.openStatus()
    else void this.openSetup()
  }

  private runtimeLock(): RuntimeLock {
    if (this.lock) return this.lock
    // From source, the lock is in the repository root's `dist/`, where
    // `cargo xtask desktop` and electron-builder.yml put it.
    const file = app.isPackaged
      ? path.join(process.resourcesPath, 'runtime-lock.json')
      : path.join(__dirname, '..', '..', 'dist', runtimeLockFile(thisPlatform))
    try {
      this.lock = readRuntimeLock(file, app.getVersion(), (at) => fs.readFileSync(at, 'utf8'))
    } catch (error) {
      if (error instanceof NoRuntimeLockError) console.error(`pagis: no Runtime Lock: ${error.detail}`)
      throw error
    }
    return this.lock
  }

  private createSupervisor(binary: string, beforeSpawn: () => void): DaemonSupervisor {
    const lock = this.runtimeLock()
    if (!fs.statSync(binary).isFile()) throw new Error('the installed Pagis server is missing')
    this.setSetupState({ kind: 'installing', detail: 'Starting the Pagis server…' })
    return new DaemonSupervisor({
      home: this.home,
      binaryPath: binary,
      version: lock.release,
      computerImage: lock.computer_image,
      pidFile: PidFile.inside(app.getPath('userData')),
      beforeSpawn,
    })
  }

  private async assertNoExternalRuntime(): Promise<void> {
    const credential = readClientCredential(this.home)
    const port = readPort(this.home)
    if (credential && await probeRuntimeIdentity(daemonUrl(port), port, credential, null, null)) {
      throw new Error('A Pagis server is still using these files. Quit it, then select Retry.')
    }
    const installedRoot = path.join(
      this.runtimeRoot, 'releases', this.runtimeLock().release,
      `${this.runtimeLock().platform}-${this.runtimeLock().arch}`,
    )
    const usesExecutable = await Promise.all([
      executableInUse(path.join(installedRoot, 'pagis')),
      executableInUse(path.join(installedRoot, 'gog')),
    ])
    if (usesExecutable.some(Boolean)) {
      throw new Error('A process is still using the installed Pagis server or helper. Quit it, then select Retry.')
    }
  }

  private async finishHandoff(url: string): Promise<void> {
    this.setupWindow?.destroy()
    this.setupWindow = null
    this.statusWindow?.destroy()
    this.statusWindow = null
    await this.openMain(url)
  }

  /**
   * Onboarding against a server this client did not start: its address,
   * and nothing else. The origin is kept so a later start opens the same
   * server. The Person signs in on that server's own page in the product
   * window, so the Session goes into the product window's cookie store
   * and nowhere else.
   */
  private async connect(url: string, signal?: AbortSignal): Promise<string> {
    this.setSetupState({ kind: 'installing', detail: 'Connecting to the Pagis server…' })
    const origin = await connectToServer(url, app.getVersion(), fetch, signal)
    this.connection = this.connections.write(origin)
    return origin
  }

  /**
   * Open the product again on the server the person signed in to
   * before. The release the administrator runs may have moved since the
   * last start, so the compatibility range is checked on every one.
   */
  private async openConnected(connection: ServerConnection): Promise<void> {
    this.connection = connection
    try {
      await assertServerIsReady(connection.origin, app.getVersion())
      await this.finishHandoff(connection.origin)
    } catch (error) {
      // The client installed nothing on this computer, so the page offers
      // to try the server again or another setup, and no repair.
      await this.openSetup()
      this.setSetupState({ kind: 'connection-failed', origin: connection.origin, reason: this.failure(error) })
    }
  }

  /** Record the release this client started, which makes it the client
   *  of its own server and of nobody else's. */
  private activateOwnRuntime(): void {
    this.runtimeState.activate(this.runtimeLock().release)
    this.connections.forget()
    this.connection = null
    this.signInWatch?.()
    this.signInWatch = null
  }

  /** The Client Credential the daemon wrote on its first run. */
  private clientCredential(): string {
    const credential = readClientCredential(this.home)
    if (!credential) throw new Error('the Pagis server did not create its client credential')
    return credential
  }

  private async openMain(url: string): Promise<void> {
    const origin = new URL(url).origin
    this.mainOrigin = origin
    if (!this.mainWindow) {
      this.mainWindow = new BrowserWindow({
        width: 1180, height: 820, title: 'Pagis', show: false, autoHideMenuBar: true,
        ...productWindowChrome(process.platform),
        webPreferences: { nodeIntegration: false, contextIsolation: true, sandbox: true },
      })
      applyProductWebRtcPolicy(this.mainWindow.webContents)
      this.mainWindow.on('close', (event) => {
        if (this.mainWindow) handleWindowClose(event, this.quitting, this.mainWindow)
      })
      this.mainWindow.on('closed', () => { this.mainWindow = null; this.mainOrigin = null })
      installNavigationRule(
        this.mainWindow.webContents,
        () => ({ window: 'product', origin: this.mainOrigin }),
        openInBrowser,
      )
      installBluetoothRefusal(this.mainWindow.webContents)
      this.mainWindow.webContents.setWindowOpenHandler(({ url: target }) => {
        // The product links an administrator to the Administration
        // Interface. On the machine that runs the server, the window of
        // its own opens it signed in, as the menu does.
        if (!this.connection && this.isAdministrationUrl(target)) {
          void this.openAdministration().catch((error: unknown) => {
            dialog.showErrorBox('Pagis administration could not be opened', this.failure(error))
          })
        } else if (opensInSystemBrowser(target, this.mainOrigin)) openInBrowser(target)
        return { action: 'deny' }
      })
    }
    // The page authenticates with the session cookie. A server this
    // client started answers the Client Credential, so the Session is in
    // the cookie jar of this window before the window loads the URL, and
    // the machine registers as its Host at once. A server this client did
    // not start shows its own sign-in page when the jar holds no Session
    // of it, and the machine registers as its Host after that sign-in.
    if (this.connection) {
      this.watchSignIn(url)
      await this.mainWindow.loadURL(url)
      this.mainWindow?.show()
    } else {
      await openSignedIn(url, this.clientCredential(), session.defaultSession.cookies, this.mainWindow)
      this.mainWindow?.show()
      this.registerHost(url)
    }
  }

  /** Register this machine as the Host of a server this client did not
   *  start once the Person signed in on its page. */
  private watchSignIn(url: string): void {
    if (this.signInWatch) return
    this.signInWatch = watchServerSignIn(url, session.defaultSession.cookies, () => this.registerHost(url))
  }

  /** The product window and the Product App origin it shows, while it is open. */
  private productWindow(): ProductWindow | null {
    if (!this.mainWindow || !this.mainOrigin) return null
    return { contents: this.mainWindow.webContents, origin: this.mainOrigin }
  }

  /**
   * Hold this machine's Host registration for as long as the client runs.
   * Each socket trades the Client Credential for a Session of its
   * own, so a reconnection after a daemon restart needs nothing the
   * person has to do.
   */
  private registerHost(url: string): void {
    if (this.hostLink) return
    this.hostLink = hostLinkFor({
      url,
      jar: session.defaultSession.cookies,
      // A server this client did not start holds no Client Credential,
      // so the person's own Session is the only one there is.
      credential: () => (this.connection ? null : this.clientCredential()),
    })
    this.hostLink.start()
  }

  private isAdministrationUrl(target: string): boolean {
    const origin = new URL(administrationUrl(readAdministrationPort(this.home))).origin
    return sameProductOrigin(origin, target)
  }

  /**
   * Open the Administration Interface. It is a second listener of
   * the same process, on the administration port, so the window trades
   * the Client Credential for a Session of its own and loads the page
   * already signed in, as the product window does. The path names the
   * view the window opens on.
   */
  private async openAdministration(view = '/'): Promise<void> {
    // The administration listener binds loopback on the machine that
    // runs the server, so it is not this machine's to open.
    if (this.connection) {
      throw new Error(
        'The administration interface answers on the Administration Port of the Pagis server ' +
        'itself, which only that machine reaches. Open it there, or through an SSH tunnel to it.',
      )
    }
    const url = new URL(view, administrationUrl(readAdministrationPort(this.home))).toString()
    // The port comes from the config file at each open, so the window
    // keeps the origin of its latest load.
    this.administrationOrigin = new URL(url).origin
    if (!this.administrationWindow) {
      this.administrationWindow = new BrowserWindow({
        width: 1100, height: 800, title: 'Pagis administration', show: false, autoHideMenuBar: true,
        webPreferences: { nodeIntegration: false, contextIsolation: true, sandbox: true },
      })
      this.administrationWindow.on('closed', () => {
        this.administrationWindow = null
        this.administrationOrigin = null
      })
      installNavigationRule(
        this.administrationWindow.webContents,
        () => ({ window: 'administration', origin: this.administrationOrigin }),
        openInBrowser,
      )
      installBluetoothRefusal(this.administrationWindow.webContents)
      this.administrationWindow.webContents.setWindowOpenHandler(({ url: target }) => {
        if (opensInSystemBrowser(target, null)) openInBrowser(target)
        return { action: 'deny' }
      })
    }
    // The administration port holds no credential exchange of its own,
    // so the Session is traded on the product port and set for this one.
    await openSignedInAt(
      url,
      daemonUrl(readPort(this.home)),
      this.clientCredential(),
      session.defaultSession.cookies,
      this.administrationWindow,
    )
    this.administrationWindow?.show()
  }

  private async openSetup(): Promise<void> {
    if (this.setupWindow) { this.setupWindow.show(); return }
    const file = path.join(__dirname, '..', 'static', 'setup.html')
    const setupUrl = pathToFileURL(file).toString()
    this.setupWindow = new BrowserWindow(
      setupWindowOptions(process.platform, path.join(__dirname, 'setupPreload.js')),
    )
    installNavigationRule(this.setupWindow.webContents, () => ({ window: 'setup', page: setupUrl }), openInBrowser)
    installBluetoothRefusal(this.setupWindow.webContents)
    this.setupWindow.webContents.setWindowOpenHandler(() => ({ action: 'deny' }))
    this.setupWindow.on('close', (event) => {
      if (this.setupWindow) handleWindowClose(event, this.quitting, this.setupWindow)
    })
    this.setupWindow.on('closed', () => { this.setupWindow = null })
    await this.setupWindow.loadFile(file)
  }

  private renderFailure(state: DaemonState): void {
    if (state.kind === 'starting') return
    if (this.setupWindow) {
      if (state.kind === 'failed') this.setSetupState({ kind: 'failed', reason: state.reason, repair: true })
      return
    }
    this.mainWindow?.hide()
    this.openStatus()
    this.statusWindow?.webContents.send('pagis:state', state)
  }

  private renderSupervisorState(state: DaemonState): void {
    const view = recoveryView(
      state,
      this.setupWindow !== null,
      this.mainWindow !== null || this.statusWindow !== null,
    )
    if (view === 'setup') {
      if (state.kind === 'failed') this.setSetupState({ kind: 'failed', reason: state.reason, repair: true })
      if (state.kind === 'taken-port') this.setSetupState(state)
      return
    }
    if (view === 'product' && state.kind === 'running') {
      void this.finishHandoff(state.url).catch((error: unknown) => this.reportHandoffFailure(error))
      return
    }
    this.renderFailure(state)
  }

  private setSetupState(state: SetupState): void {
    this.setupState = state
    this.setupWindow?.webContents.send('pagis:setup-state', state)
  }

  private openStatus(): void {
    if (this.statusWindow) { this.statusWindow.show(); return }
    const file = path.join(__dirname, '..', 'static', 'status.html')
    const statusUrl = pathToFileURL(file).toString()
    this.statusWindow = new BrowserWindow(
      statusWindowOptions(process.platform, path.join(__dirname, 'statusPreload.js')),
    )
    installNavigationRule(this.statusWindow.webContents, () => ({ window: 'status', page: statusUrl }), openInBrowser)
    installBluetoothRefusal(this.statusWindow.webContents)
    this.statusWindow.on('close', (event) => {
      if (this.statusWindow) handleWindowClose(event, this.quitting, this.statusWindow)
    })
    this.statusWindow.on('closed', () => { this.statusWindow = null })
    void this.statusWindow.loadFile(file)
  }

  private handleRequests(): void {
    ipcMain.handle('pagis:setup-state-please', (event) => {
      if (!this.trustedSetup(event)) throw new Error('untrusted setup request')
      this.setupWindow?.webContents.send('pagis:setup-state', this.setupState)
    })
    ipcMain.handle('pagis:install', (event, request: unknown) => {
      if (!this.trustedSetup(event)) throw new Error('untrusted setup request')
      return this.controller.run(request).catch((error: unknown) => {
        throw isServerRequest(request) ? this.serverCheckFailure(error) : this.setupFailure(error)
      })
    })
    ipcMain.handle('pagis:cancel-setup', async (event) => {
      if (!this.trustedSetup(event)) throw new Error('untrusted setup request')
      await this.controller.cancel()
      this.setSetupState({ kind: 'ready' })
    })
    ipcMain.handle('pagis:setup-use-port', async (event, port: unknown) => {
      if (!this.trustedSetup(event) || !Number.isInteger(port) || (port as number) < 1 || (port as number) > 65535) throw new Error('invalid port request')
      this.setSetupState({ kind: 'installing', detail: `Starting Pagis on port ${String(port)}…` })
      return this.controller.usePortAndResume(port as number).catch((error: unknown) => {
        throw this.setupFailure(error)
      })
    })
    ipcMain.handle('pagis:retry-server', async (event) => {
      if (!this.trustedSetup(event) || !this.connection) throw new Error('untrusted setup request')
      this.setSetupState({ kind: 'installing', detail: 'Connecting to the Pagis server…' })
      await this.openConnected(this.connection)
    })
    ipcMain.handle('pagis:state-please', (event) => {
      if (!this.trustedStatus(event)) throw new Error('untrusted status request')
      this.statusWindow?.webContents.send('pagis:state', this.controller.state)
    })
    ipcMain.handle('pagis:retry', (event) => {
      if (!this.trustedStatus(event)) throw new Error('untrusted status request')
      return this.controller.retry()
    })
    ipcMain.handle('pagis:use-port', (event, port: unknown) => {
      if (!this.trustedStatus(event) || !Number.isInteger(port) || (port as number) < 1 || (port as number) > 65535) throw new Error('invalid port request')
      const state = this.controller.state
      if (!state || state.kind === 'running') throw new Error('the Pagis server does not need another port')
      return this.controller.usePortAndResume(port as number)
    })
    ipcMain.handle('pagis:reveal-logs', (event) => {
      if (!this.trustedStatus(event)) throw new Error('untrusted status request')
      return shell.openPath(path.join(this.home, 'logs'))
    })
    ipcMain.handle('pagis:quit', (event) => {
      if (!this.trustedSetup(event) && !this.trustedStatus(event)) throw new Error('untrusted quit request')
      return this.requestQuit()
    })
  }

  private trustedSetup(event: Electron.IpcMainInvokeEvent): boolean {
    if (!this.setupWindow) return false
    return isTrustedSetupRequest(event, this.setupWindow.webContents, pathToFileURL(path.join(__dirname, '..', 'static', 'setup.html')).toString())
  }

  private trustedStatus(event: Electron.IpcMainInvokeEvent): boolean {
    if (!this.statusWindow) return false
    return isTrustedSetupRequest(event, this.statusWindow.webContents, pathToFileURL(path.join(__dirname, '..', 'static', 'status.html')).toString())
  }

  private async checkVersion(): Promise<void> {
    this.update = await checkForNewVersion(app.getVersion())
    if (this.update && this.tray) renderTray(this.tray, this.menuActions())
  }

  /** A handoff that no setup job awaits still has to reach the person. */
  private reportHandoffFailure(error: unknown): void {
    dialog.showErrorBox('Pagis could not be opened', this.failure(error))
  }

  private failure(error: unknown): string {
    return redact(installationError(error).message)
  }

  private setupFailure(error: unknown): Error {
    this.setSetupState(this.failureState(error))
    return new Error(this.failure(error))
  }

  /** A check of the server address that failed. The client installed
   *  nothing for it, so the page shows the reason under the address and
   *  offers no repair. */
  private serverCheckFailure(error: unknown): Error {
    const reason = this.failure(error)
    this.setSetupState({ kind: 'server-check-failed', reason })
    return new Error(reason)
  }

  /**
   * A failure of this computer's installation as the setup page shows
   * it: the taken-port page when the server found its port taken, else
   * the reason. Repair is offered only where this computer holds an
   * installation, and never on a client with no Runtime Lock.
   */
  private failureState(error: unknown): SetupState {
    const repair = this.installedHere() && !(error instanceof NoRuntimeLockError)
    return setupFailureState(this.failure(error), this.controller.state, repair)
  }

  /** Whether a release was installed and started on this computer. State
   *  files that the client cannot read also count, because a repair
   *  writes them again. */
  private installedHere(): boolean {
    try {
      return this.runtimeState.releaseToStart() !== null
    } catch {
      return true
    }
  }

  private menuActions() {
    return {
      open: () => this.show(),
      openAdministration: () => void this.openAdministration().catch((error: unknown) => {
        dialog.showErrorBox('Pagis administration could not be opened', this.failure(error))
      }),
      quit: () => void this.requestQuit(),
      openAtLogin: (open: boolean) => loginItem.setOpenAtLogin(open),
      isOpenAtLogin: () => loginItem.isOpenAtLogin(),
      newVersion: () => this.update,
    }
  }
}

/** Open an address in the system browser, or in the mail program for a
 *  `mailto:` address. */
function openInBrowser(url: string): void {
  void shell.openExternal(url)
}

/** macOS keeps login items itself; Linux reads an XDG autostart entry. */
const loginItem: LoginItem = process.platform === 'linux'
  ? new AutostartEntry()
  : {
      isOpenAtLogin: () => app.getLoginItemSettings().openAtLogin,
      setOpenAtLogin: (open) => app.setLoginItemSettings({ openAtLogin: open }),
    }

function run(): void {
  if (SMOKE) {
    smokeRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-client-smoke-'))
    app.setPath('userData', path.join(smokeRoot, 'client'))
    process.env.PAGIS_HOME = path.join(smokeRoot, 'workspace')
  }
  if (!app.requestSingleInstanceLock()) {
    app.exit(SMOKE ? 1 : 0)
    return
  }
  const shellApp = new Shell()
  reportUncaughtExceptions(process, (error) => shellApp.showUncaught(error))
  app.on('second-instance', () => shellApp.show())
  app.on('activate', () => shellApp.show())
  app.on('window-all-closed', () => {})
  endOnQuit(app, () => shellApp.stop())
  app.whenReady().then(() => shellApp.start()).catch((error: unknown) => {
    dialog.showErrorBox('Pagis could not start', String(error)); app.exit(1)
  })
  if (SMOKE) setTimeout(() => { console.error('pagis smoke: setup did not open in time'); app.exit(1) }, SMOKE_DEADLINE_MS).unref()
}

run()
