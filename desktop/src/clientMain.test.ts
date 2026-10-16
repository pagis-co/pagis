import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'
import { pathToFileURL } from 'node:url'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import type { MenuActions } from './menus'

const mocks = vi.hoisted(() => ({
  changed: null as (() => void) | null,
  root: '', ready: null as Promise<void> | null, ipc: new Map<string, (...args: any[]) => any>(), events: new Map<string, (...args: any[]) => any>(),
  actions: null as MenuActions | null, setup: null as any, opened: [] as string[], external: vi.fn(), protocol: vi.fn(),
}))
vi.mock('electron', async () => {
  const { EventEmitter } = await import('node:events')
  return {
    app: {
      exit: vi.fn(), getPath: () => mocks.root, getVersion: () => '0.3.0', isPackaged: false,
      requestSingleInstanceLock: () => true, setAsDefaultProtocolClient: mocks.protocol,
      whenReady: () => mocks.ready ?? Promise.resolve(), on: (name: string, handler: any) => mocks.events.set(name, handler),
      getLoginItemSettings: () => ({ openAtLogin: false }),
    },
    Menu: { setApplicationMenu: vi.fn() },
    ipcMain: { handle: (name: string, handler: any) => mocks.ipc.set(name, handler) },
    session: { defaultSession: { setPermissionRequestHandler: vi.fn(), setPermissionCheckHandler: vi.fn(), setDevicePermissionHandler: vi.fn(), setBluetoothPairingHandler: vi.fn() } },
    shell: { openExternal: mocks.external }, dialog: { showErrorBox: vi.fn() },
    BrowserWindow: class extends EventEmitter {
      webContents = Object.assign(new EventEmitter(), { mainFrame: { url: '' }, setWindowOpenHandler: vi.fn(), send: vi.fn() })
      show = vi.fn(); focus = vi.fn(); destroy = vi.fn(); isVisible = () => true
      async loadFile(file: string) { this.webContents.mainFrame.url = pathToFileURL(file).toString() }
      constructor() { super(); Object.assign(this.webContents, { getURL: () => this.webContents.mainFrame.url }); mocks.setup = this }
    },
  }
})
vi.mock('./uncaughtFailure', () => ({ reportUncaughtExceptions: vi.fn() }))
vi.mock('./menus', () => ({
  applicationMenu: (actions: MenuActions) => { mocks.actions = actions }, createTray: () => ({}), renderTray: vi.fn(),
}))
vi.mock('./serverWindows', () => ({ ServerWindows: class {
  constructor(_connections: unknown, actions: { changed(): void }) { mocks.changed = actions.changed }
  currentId: string | null = null
  open = vi.fn(async (server: any, url: string) => { this.currentId = server.id; mocks.opened.push(url) })
  show = () => false
  visible = () => undefined
  codingSessions = () => 0
  exitTraffic = () => []
  acceptSignIn = async () => false
  beginSignIn = (server: any) => `${server.origin}sign-in?app=desktop`
  remove = async () => {}
  takeVerifier = () => undefined
} }))
beforeEach(async () => {
  vi.resetModules()
  mocks.root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-client-main-'))
  mocks.ready = null; mocks.ipc.clear(); mocks.events.clear(); mocks.actions = null; mocks.opened.length = 0; mocks.external.mockClear()
  vi.stubEnv('PAGIS_HOME', path.join(mocks.root, 'home'))
  vi.stubGlobal('fetch', vi.fn(async (url: URL) => String(url).endsWith('/health')
    ? new Response(JSON.stringify({ status: 'ok', version: '0.3.0' }))
    : new Response(null, { status: 410 })))
  await import('./clientMain')
  await vi.waitFor(() => expect(mocks.setup?.webContents.mainFrame.url).toContain('setup.html'))
})
afterEach(() => { vi.unstubAllGlobals(); vi.unstubAllEnvs(); fs.rmSync(mocks.root, { recursive: true, force: true }) })
function event() { return { sender: mocks.setup.webContents, senderFrame: mocks.setup.webContents.mainFrame } }

it('keeps both connections and updates the menus in a run from source', async () => {
  await mocks.ipc.get('pagis:install')!(event(), { kind: 'server', url: 'home.example' })
  mocks.actions!.addServer()
  await vi.waitFor(() => expect(mocks.setup.webContents.mainFrame.url).toContain('setup.html'))
  await mocks.ipc.get('pagis:install')!(event(), { kind: 'server', url: 'work.example' })
  expect(mocks.opened).toEqual(['https://home.example/', 'https://work.example/'])
  expect(mocks.actions!.servers().map(({ label }) => label)).toEqual(['home.example', 'work.example'])
  expect(fs.readdirSync(mocks.root)).toContain('servers.json')
  expect(fs.readdirSync(mocks.root)).not.toContain('server.json')
})

it('uses trusted setup IPC for browser sign-in and registers the link scheme', async () => {
  await expect(mocks.ipc.get('pagis:browser-sign-in')!({ sender: {}, senderFrame: {} }, 'home.example')).rejects.toThrow('untrusted')
  expect(mocks.external).not.toHaveBeenCalled()
  await mocks.ipc.get('pagis:browser-sign-in')!(event(), 'home.example')
  expect(mocks.external).toHaveBeenCalledWith('https://home.example/sign-in?app=desktop')
  expect(mocks.protocol).toHaveBeenCalledWith('pagis')
})

it('opens nothing for rejected macOS and Linux links and shows the app for a normal second start', async () => {
  const shown = mocks.setup.show.mock.calls.length
  const prevented = vi.fn()
  mocks.events.get('open-url')!({ preventDefault: prevented }, 'pagis://other')
  mocks.events.get('second-instance')!({}, ['pagis-client', 'pagis://auth?state=unknown#secret'])
  await Promise.resolve(); await Promise.resolve()
  expect(prevented).toHaveBeenCalledOnce()
  expect(mocks.setup.show).toHaveBeenCalledTimes(shown)
  expect(mocks.opened).toEqual([])
  mocks.events.get('second-instance')!({}, ['pagis-client'])
  expect(mocks.setup.show).toHaveBeenCalledTimes(shown + 1)
})

it('opens no setup or product window for a cold Linux protocol start', async () => {
  fs.rmSync(mocks.root, { recursive: true, force: true })
  mocks.root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-cold-link-'))
  mocks.setup = null
  mocks.opened.length = 0
  vi.resetModules()
  const argv = process.argv
  process.argv = [...argv, 'pagis://auth?state=unknown#secret']
  try {
    await import('./clientMain')
    await new Promise((resolve) => setTimeout(resolve, 20))
    expect(mocks.setup).toBeNull()
    expect(mocks.opened).toEqual([])
  } finally { process.argv = argv }
})

it('opens no setup or product window for a cold macOS open-url event', async () => {
  mocks.setup = null
  vi.resetModules()
  let ready!: () => void
  mocks.ready = new Promise<void>((resolve) => { ready = resolve })
  await import('./clientMain')
  const prevented = vi.fn()
  mocks.events.get('open-url')!({ preventDefault: prevented }, 'pagis://auth?state=unknown#secret')
  ready()
  await new Promise((resolve) => setTimeout(resolve, 20))
  expect(prevented).toHaveBeenCalledOnce()
  expect(mocks.setup).toBeNull()
  expect(mocks.opened).toEqual([])
})

it('keeps a Local Installation separate from connected servers', async () => {
  const runtime = path.join(mocks.root, 'runtime')
  fs.mkdirSync(runtime, { recursive: true })
  fs.writeFileSync(path.join(runtime, 'active.json'), JSON.stringify({ release: '0.3.0', platform: process.platform, arch: process.arch }))
  expect(mocks.actions!.canAddServer()).toBe(false)
  await expect(mocks.ipc.get('pagis:browser-sign-in')!(event(), 'home.example')).rejects.toThrow('Local Installation')
  expect(mocks.external).not.toHaveBeenCalled()
  expect(mocks.opened).toEqual([])
})

it('refuses local setup while it keeps connected servers', async () => {
  await mocks.ipc.get('pagis:install')!(event(), { kind: 'server', url: 'home.example' })
  mocks.actions!.addServer()
  await vi.waitFor(() => expect(mocks.setup.webContents.mainFrame.url).toContain('setup.html'))
  expect(() => mocks.ipc.get('pagis:install')!(event(), { kind: 'local', people: 'one' })).toThrow('Remove connected servers')
})

it('refuses stored connections beside a Local Installation at start', async () => {
  const { ServerConnections } = await import('./serverConnection')
  new ServerConnections(mocks.root).add('home.example')
  fs.writeFileSync(path.join(mocks.root, 'runtime', 'active.json'), JSON.stringify({ release: '0.3.0', platform: process.platform, arch: process.arch }))
  mocks.setup = null
  vi.resetModules()
  await import('./clientMain')
  await new Promise((resolve) => setTimeout(resolve, 20))
  expect(mocks.opened).toEqual([])
  expect(mocks.setup.webContents.send).toHaveBeenCalledWith('pagis:setup-state', expect.objectContaining({ kind: 'failed', reason: expect.stringContaining('Local Installation') }))
})

it.each([301, 308])('stores a %s move at start and starts at the saved new origin next time', async (status) => {
  const { ServerConnections } = await import('./serverConnection')
  const connections = new ServerConnections(mocks.root)
  const before = connections.add('https://old.example/')
  const request = vi.fn(async (url: URL) => new URL(url).hostname === 'old.example'
    ? new Response(null, { status, headers: { Location: 'https://new.example/api/v1/health' } })
    : Response.json({ status: 'ok', version: '0.3.0' }))
  vi.stubGlobal('fetch', request)
  mocks.opened.length = 0
  vi.resetModules()
  await import('./clientMain')
  await vi.waitFor(() => expect(mocks.opened).toEqual(['https://new.example/']))
  expect(connections.list()).toEqual([{ ...before, origin: 'https://new.example/', label: 'new.example' }])
  mocks.opened.length = 0
  request.mockClear()
  vi.resetModules()
  await import('./clientMain')
  await vi.waitFor(() => expect(mocks.opened).toEqual(['https://new.example/']))
  expect(request.mock.calls.every(([url]) => new URL(url).hostname === 'new.example')).toBe(true)
})

it('refreshes the server menu when a reload changes the Public Origin', async () => {
  await mocks.ipc.get('pagis:install')!(event(), { kind: 'server', url: 'old.example' })
  const { ServerConnections } = await import('./serverConnection')
  const connections = new ServerConnections(mocks.root)
  const before = connections.list()[0]
  connections.followRedirect(before.id, 301, before.origin, 'https://new.example/')
  mocks.changed!()
  expect(mocks.actions!.servers()).toEqual([{ ...before, origin: 'https://new.example/', label: 'new.example' }])
})
