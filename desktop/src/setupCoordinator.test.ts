import { describe, expect, it } from 'vitest'
import * as fs from 'node:fs'
import * as os from 'node:os'
import * as path from 'node:path'

import { RuntimeState } from './runtimeState'
import { isServerRequest, SetupCoordinator } from './setupCoordinator'

const LOCAL = { kind: 'local', people: 'one' }

describe('client setup', () => {
  it('installs one job for one Person, proves the server, then activates', async () => {
    const calls: string[] = []
    let finishInstall!: (binary: string) => void
    const installing = new Promise<string>((resolve) => { finishInstall = resolve })
    const setup = new SetupCoordinator({
      install: async () => { calls.push('install'); return installing },
      start: async (binary) => { calls.push(`start:${binary}`); return 'http://127.0.0.1:4400/' },
      activate: () => calls.push('activate'),
      openProduct: async () => { calls.push('open-product') },
      connect: async () => 'https://pagis.example.com/',
      openMultiUserSwitch: async () => { calls.push('open-multi-user-switch') },
    })

    const first = setup.run(LOCAL)
    const duplicate = setup.run(LOCAL)
    expect(calls).toEqual(['install'])
    finishInstall('/installed/pagis')
    await Promise.all([first, duplicate])

    expect(calls).toEqual([
      'install', 'start:/installed/pagis', 'activate', 'open-product',
    ])
  })

  it('installs for several People, then opens the Multi-User Mode switch after the product', async () => {
    const calls: string[] = []
    const setup = new SetupCoordinator({
      install: async () => { calls.push('install'); return '/installed/pagis' },
      start: async (binary) => { calls.push(`start:${binary}`); return 'http://127.0.0.1:4400/' },
      activate: () => calls.push('activate'),
      openProduct: async (url) => { calls.push(`open-product:${url}`) },
      connect: async () => { calls.push('connect'); return 'https://pagis.example.com/' },
      openMultiUserSwitch: async () => { calls.push('open-multi-user-switch') },
    })

    await setup.run({ kind: 'local', people: 'several' })

    expect(calls).toEqual([
      'install', 'start:/installed/pagis', 'activate',
      'open-product:http://127.0.0.1:4400/', 'open-multi-user-switch',
    ])
  })

  it('does not open the Multi-User Mode switch after a cancelled installation', async () => {
    const calls: string[] = []
    let finishOpen!: () => void
    const setup = new SetupCoordinator({
      install: async () => '/installed/pagis',
      cancel: () => { calls.push('cancel'); finishOpen() },
      start: async () => 'http://127.0.0.1:4400/',
      activate: () => {},
      openProduct: () => new Promise((resolve) => { finishOpen = resolve }),
      connect: async () => 'https://pagis.example.com/',
      openMultiUserSwitch: async () => { calls.push('open-multi-user-switch') },
    })
    const job = setup.run({ kind: 'local', people: 'several' })
    while (!finishOpen) await Promise.resolve()

    const cancel = setup.cancel()
    await expect(job).rejects.toThrow(/cancelled/)
    await cancel

    expect(calls).toEqual(['cancel'])
  })

  /** Only a page that skips its own checks sends such a request, and
   *  the setup page shows the refusal as it is: in words for a person. */
  it('rejects an unknown kind, remote and extra native arguments in words for a person', async () => {
    const setup = new SetupCoordinator({
      install: async () => '/installed/pagis',
      start: async () => 'http://127.0.0.1:4400/', activate: () => {},
      openProduct: async () => {},
      connect: async () => 'https://pagis.example.com/',
      openMultiUserSwitch: async () => {},
    })
    for (const request of [
      { kind: 'connected' },
      { kind: 'local', people: 'one', url: 'https://remote.example' }, {}, null, [],
      'local', { kind: 'local' }, { kind: 'local', people: 'all' },
      { kind: 'local', people: 2 }, { kind: 'local', people: null },
      { kind: 'local', people: ['several'] },
    ]) {
      await expect(setup.run(request)).rejects.toThrow('Pagis does not know this setup. Choose a setup again.')
    }
  })

  it('cancels one in-progress installation before Quit completes', async () => {
    let rejectInstall!: (error: Error) => void
    const calls: string[] = []
    const setup = new SetupCoordinator({
      install: () => new Promise((_resolve, reject) => { rejectInstall = reject }),
      cancel: () => { calls.push('cancel'); rejectInstall(new Error('cancelled')) },
      start: async () => '',
      activate: () => {}, openProduct: async () => {},
      connect: async () => 'https://pagis.example.com/',
      openMultiUserSwitch: async () => {},
    })
    void setup.run(LOCAL).catch(() => undefined)

    await setup.cancel()

    expect(calls).toEqual(['cancel'])
  })

  it('does not launch after a completed download is cancelled', async () => {
    let finishInstall!: (binary: string) => void
    const calls: string[] = []
    const setup = new SetupCoordinator({
      install: () => new Promise((resolve) => { finishInstall = resolve }),
      cancel: () => { calls.push('cancel') },
      start: async () => '',
      activate: () => {}, openProduct: async () => {},
      connect: async () => 'https://pagis.example.com/',
      openMultiUserSwitch: async () => {},
    })
    const job = setup.run(LOCAL)
    finishInstall('/installed/pagis')
    const cancel = setup.cancel()

    await expect(job).rejects.toThrow(/cancelled/)
    await cancel
    expect(calls).toEqual(['cancel'])
  })

  it('stops first startup and prevents a late activation or window', async () => {
    let finishStart!: (url: string) => void
    const calls: string[] = []
    const setup = new SetupCoordinator({
      install: async () => '/installed/pagis',
      cancel: () => { calls.push('stop-owned-child'); finishStart('http://127.0.0.1:4400/') },
      start: () => new Promise((resolve) => { finishStart = resolve }),
      activate: () => calls.push('activate'), openProduct: async () => { calls.push('open-product') },
      connect: async () => 'https://pagis.example.com/',
      openMultiUserSwitch: async () => {},
    })
    const job = setup.run(LOCAL)
    while (!finishStart) await Promise.resolve()

    const cancel = setup.cancel()
    await expect(job).rejects.toThrow(/cancelled/)
    await cancel

    expect(calls).toEqual(['stop-owned-child'])
  })

  it('keeps the attempted release marker after first startup is interrupted', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pagis-first-start-'))
    const state = new RuntimeState(root)
    state.activate('0.1.0')
    const setup = new SetupCoordinator({
      install: async () => '/installed/pagis',
      start: async () => { state.beginLaunch('0.2.0'); throw new Error('startup interrupted') },
      activate: () => state.activate('0.2.0'), openProduct: async () => {},
      connect: async () => 'https://pagis.example.com/',
      openMultiUserSwitch: async () => {},
    })

    await expect(setup.run(LOCAL)).rejects.toThrow(/interrupted/)

    expect(state.active()?.release).toBe('0.1.0')
    expect(state.releaseToStart()).toBe('0.2.0')
    fs.rmSync(root, { recursive: true, force: true })
  })
})

describe('setup against a server the client did not start', () => {
  const SERVER = { kind: 'server', url: 'pagis.example.com' }

  it('connects and opens the product, and installs nothing', async () => {
    const calls: string[] = []
    const setup = new SetupCoordinator({
      install: async () => { calls.push('install'); return '/installed/pagis' },
      start: async () => { calls.push('start'); return '' },
      activate: () => calls.push('activate'),
      openProduct: async (url) => { calls.push(`open-product:${url}`) },
      connect: async (url) => { calls.push(`connect:${url}`); return 'https://pagis.example.com/' },
      openMultiUserSwitch: async () => {},
    })

    await setup.run(SERVER)

    expect(calls).toEqual([
      'connect:pagis.example.com',
      'open-product:https://pagis.example.com/',
    ])
  })

  /** The Person signs in on the server's own page, so a request that
   *  carries an email address or a password is not a setup request. */
  it('refuses a request with no address, or with anything besides the address', async () => {
    const setup = new SetupCoordinator({
      install: async () => '/installed/pagis', start: async () => '',
      activate: () => {}, openProduct: async () => {},
      connect: async () => 'https://pagis.example.com/',
      openMultiUserSwitch: async () => {},
    })
    for (const request of [
      { kind: 'server' },
      { kind: 'server', url: '  ' },
      { kind: 'server', url: 1 },
      { ...SERVER, email: 'ada@example.com', password: 'correct horse battery' },
      { ...SERVER, password: 'correct horse battery' },
      { ...SERVER, people: 'several' },
    ]) {
      await expect(setup.run(request)).rejects.toThrow('Enter the address of your Pagis server, then select Continue.')
    }
  })

  it('tells a request for a server from a request for this computer, valid or not', () => {
    expect(isServerRequest(SERVER)).toBe(true)
    expect(isServerRequest({ kind: 'server' })).toBe(true)
    for (const request of [LOCAL, { kind: 'local' }, {}, null, [], 'server']) {
      expect(isServerRequest(request)).toBe(false)
    }
  })

  it('does not open the product after a cancelled connection', async () => {
    const calls: string[] = []
    let finishConnect!: (origin: string) => void
    const setup = new SetupCoordinator({
      install: async () => '/installed/pagis', start: async () => '',
      activate: () => {},
      openProduct: async () => { calls.push('open-product') },
      cancel: () => { calls.push('cancel'); finishConnect('https://pagis.example.com/') },
      connect: () => new Promise((resolve) => { finishConnect = resolve }),
      openMultiUserSwitch: async () => {},
    })
    const job = setup.run(SERVER)
    while (!finishConnect) await Promise.resolve()

    const cancel = setup.cancel()
    await expect(job).rejects.toThrow(/cancelled/)
    await cancel

    expect(calls).toEqual(['cancel'])
  })
})
