import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ServerWindows } from './serverWindows'
import { ServerConnections } from './serverConnection'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

const mocks = vi.hoisted(() => ({ partitions: new Map<string, any>(), windows: [] as any[], links: [] as any[] }))
vi.mock('electron', async () => {
  const { EventEmitter } = await import('node:events')
  return {
    session: { fromPartition: (partition: string) => {
      if (!mocks.partitions.has(partition)) {
        const cookies = Object.assign(new EventEmitter(), {
          held: [] as any[],
          get: vi.fn(async () => cookies.held),
          set: vi.fn(async (cookie: any) => { cookies.held = [cookie]; cookies.emit('changed', {}, cookie, 'explicit', false) }),
        })
        mocks.partitions.set(partition, {
          cookies, clearStorageData: vi.fn(async () => { cookies.held = [] }), clearCache: vi.fn(async () => {}),
          setPermissionRequestHandler: vi.fn(), setPermissionCheckHandler: vi.fn(), setDevicePermissionHandler: vi.fn(), setBluetoothPairingHandler: vi.fn(),
        })
      }
      return mocks.partitions.get(partition)
    } },
    BrowserWindow: class extends EventEmitter {
      webContents = Object.assign(new EventEmitter(), { setWindowOpenHandler: vi.fn(), setWebRTCIPHandlingPolicy: vi.fn(), mainFrame: { url: '', origin: '' } })
      loadURL = vi.fn(async (url: string) => { this.webContents.mainFrame.url = url; this.webContents.mainFrame.origin = new URL(url).origin })
      show = vi.fn()
      focus = vi.fn(() => this.emit('focus'))
      hide = vi.fn()
      isVisible = () => true
      destroy = vi.fn(() => this.emit('closed'))
      constructor(readonly options: any) { super(); mocks.windows.push(this) }
    },
  }
})
vi.mock('./serverHost', () => ({ hostLinkFor: vi.fn((deps) => {
  const link = { deps, start: vi.fn(), stop: vi.fn(async () => {}), codingSessions: () => 2 }
  mocks.links.push(link)
  return link
}) }))

let root: string
let connections: ServerConnections
let windows: ServerWindows
beforeEach(() => {
  mocks.partitions.clear(); mocks.windows.length = 0; mocks.links.length = 0
  root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-server-windows-'))
  connections = new ServerConnections(root)
  windows = new ServerWindows(connections, { changed: vi.fn(), openExternal: vi.fn(), quitting: () => false })
})
afterEach(async () => { await windows.stop(); fs.rmSync(root, { recursive: true, force: true }) })

describe('one window and session for each server', () => {
  it('uses two persistent partitions and keeps cookies, storage, watches and Host links apart even for two ports of one host', async () => {
    const home = connections.add('https://pagis.example:4400/')
    const work = connections.add('https://pagis.example:4401/')
    await windows.open(home, home.origin)
    await windows.open(work, work.origin)
    expect(mocks.windows.map((w) => w.options.webPreferences.partition)).toEqual([`persist:server-${home.id}`, `persist:server-${work.id}`])
    const homeSession = mocks.partitions.get(`persist:server-${home.id}`)
    const workSession = mocks.partitions.get(`persist:server-${work.id}`)
    await homeSession.cookies.set({ name: 'pagis_session', value: 'home-session', secure: true })
    await vi.waitFor(() => expect(mocks.links).toHaveLength(1))
    expect(await workSession.cookies.get({ name: 'pagis_session', url: work.origin })).toEqual([])
    expect(mocks.links[0].deps.jar).toBe(homeSession.cookies)
    expect(mocks.links[0].deps.url).toBe(home.origin)
    await workSession.cookies.set({ name: 'pagis_session', value: 'work-session', secure: true })
    await vi.waitFor(() => expect(mocks.links).toHaveLength(2))
    expect(mocks.links[1].deps.jar).toBe(workSession.cookies)
    expect(mocks.links[0].deps.traffic).not.toBe(mocks.links[1].deps.traffic)
    expect(windows.codingSessions()).toBe(4)
    await windows.open(home, home.origin)
    expect(mocks.windows).toHaveLength(2)
    expect(homeSession.cookies.listenerCount('changed')).toBe(1)
  })

  it('stops only the removed server, clears its storage and cache and removes its record', async () => {
    const home = connections.add('home.example')
    const work = connections.add('work.example')
    await windows.open(home, home.origin); await windows.open(work, work.origin)
    const held = mocks.partitions.get(`persist:server-${home.id}`)
    await held.cookies.set({ name: 'pagis_session', value: 'session', secure: true })
    await vi.waitFor(() => expect(mocks.links).toHaveLength(1))
    await windows.remove(home.id)
    expect(mocks.links[0].stop).toHaveBeenCalledOnce()
    expect(held.clearStorageData).toHaveBeenCalledOnce()
    expect(held.clearCache).toHaveBeenCalledOnce()
    expect(held.cookies.listenerCount('changed')).toBe(0)
    expect(connections.list()).toEqual([work])
    expect(mocks.windows[0].destroy).toHaveBeenCalledOnce()
    expect(mocks.windows[1].destroy).not.toHaveBeenCalled()
  })

  it('opens a good callback in its server window and gives its main frame the verifier once', async () => {
    const home = connections.add('home.example')
    const work = connections.add('work.example')
    await windows.open(work, work.origin)
    const browser = new URL(windows.beginSignIn(home))
    const link = `pagis://auth?state=${browser.searchParams.get('state')}#secret`
    expect(await windows.acceptSignIn(link)).toBe(true)
    expect(mocks.windows[1].loadURL).toHaveBeenCalledWith('https://home.example/sign-in#secret')
    expect(await windows.acceptSignIn(link)).toBe(false)
    const contents = mocks.windows[1].webContents
    expect(windows.takeVerifier({ sender: mocks.windows[0].webContents, senderFrame: mocks.windows[0].webContents.mainFrame }, 'secret')).toBeUndefined()
    expect(windows.takeVerifier({ sender: contents, senderFrame: { ...contents.mainFrame } }, 'secret')).toBeUndefined()
    expect(windows.takeVerifier({ sender: contents, senderFrame: contents.mainFrame }, 'wrong')).toBeUndefined()
    expect(windows.takeVerifier({ sender: contents, senderFrame: contents.mainFrame }, 'secret')).toMatch(/^[A-Za-z0-9_-]{43}$/)
    expect(windows.takeVerifier({ sender: contents, senderFrame: contents.mainFrame }, 'secret')).toBeUndefined()
    expect(mocks.windows[0].loadURL).toHaveBeenCalledTimes(1)
  })

  it('opens no window for an unknown link or for a removed server', async () => {
    const home = connections.add('home.example')
    const state = new URL(windows.beginSignIn(home)).searchParams.get('state')
    await windows.remove(home.id)
    expect(await windows.acceptSignIn(`pagis://auth?state=${state}#secret`)).toBe(false)
    expect(await windows.acceptSignIn('pagis://other')).toBe(false)
    expect(mocks.windows).toEqual([])
  })
})

it('holds the verifier before the sign-in page starts to load', async () => {
  const home = connections.add('home.example')
  await windows.open(home, home.origin)
  const native = mocks.windows[0]
  const browser = new URL(windows.beginSignIn(home))
  let verifier: string | undefined
  native.loadURL.mockImplementationOnce(async (url: string) => {
    native.webContents.mainFrame.url = url
    verifier = windows.takeVerifier({ sender: native.webContents, senderFrame: native.webContents.mainFrame }, 'secret')
  })
  await windows.acceptSignIn(`pagis://auth?state=${browser.searchParams.get('state')}#secret`)
  expect(verifier).toMatch(/^[A-Za-z0-9_-]{43}$/)
})

it('refuses a verifier on another origin or page and after five minutes', async () => {
  vi.useFakeTimers()
  try {
    const home = connections.add('home.example')
    const state = new URL(windows.beginSignIn(home)).searchParams.get('state')
    await windows.acceptSignIn(`pagis://auth?state=${state}#secret`)
    const contents = mocks.windows[0].webContents
    for (const url of ['https://evil.example/sign-in', 'https://home.example/settings']) {
      contents.mainFrame.url = url
      expect(windows.takeVerifier({ sender: contents, senderFrame: contents.mainFrame }, 'secret')).toBeUndefined()
    }
    contents.mainFrame.url = 'https://home.example/sign-in'
    vi.advanceTimersByTime(300_000)
    expect(windows.takeVerifier({ sender: contents, senderFrame: contents.mainFrame }, 'secret')).toBeUndefined()
  } finally { vi.useRealTimers() }
})
