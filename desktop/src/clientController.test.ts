import { EventEmitter } from 'node:events'

import { describe, expect, it } from 'vitest'

import { ClientController, type ClientSupervisor } from './clientController'
import type { DaemonState } from './daemon'

class FakeSupervisor extends EventEmitter implements ClientSupervisor {
  state: DaemonState = { kind: 'starting' }
  starts = 0
  stops = 0

  async start(): Promise<void> {
    this.starts += 1
    this.state = { kind: 'running', url: 'http://127.0.0.1:4400/', port: 4400, version: '0.1.0', owned: true }
    this.emit('state', this.state)
  }

  async stop(): Promise<void> { this.stops += 1 }
  async usePort(port: number): Promise<void> {
    this.state = { kind: 'running', url: `http://127.0.0.1:${port}/`, port, version: '0.1.0', owned: true }
    this.emit('state', this.state)
  }
}

describe('client setup and recovery controller', () => {
  it('cancels automatic resume before it can spawn or open a window', async () => {
    let rejectInstall!: (error: Error) => void
    let created = 0
    const controller = new ClientController({
      backUp: async () => {},
      install: ({ signal }) => new Promise((_resolve, reject) => {
        rejectInstall = reject
        signal?.addEventListener('abort', () => reject(new DOMException('cancelled', 'AbortError')))
      }),
      beginLaunch: () => {}, connect: async () => '', activate: () => {}, openProduct: async () => {},
      createSupervisor: () => { created += 1; return new FakeSupervisor() },
      assertNoExternalRuntime: async () => {}, onDaemonState: () => {},
      openMultiUserSwitch: async () => {},
    })
    const resume = controller.resume()

    await controller.cancel()

    await expect(resume).rejects.toThrow(/cancelled/)
    expect(rejectInstall).toBeTypeOf('function')
    expect(created).toBe(0)
  })

  it('retains the owned supervisor across a failed handoff, Retry and Quit', async () => {
    const supervisor = new FakeSupervisor()
    let creates = 0
    let opens = 0
    const controller = new ClientController({
      backUp: async () => {},
      install: async () => '/installed/pagis', beginLaunch: () => {},
      connect: async () => '',
      activate: () => {},
      openProduct: async () => { opens += 1; if (opens === 1) throw new Error('handoff failed') },
      createSupervisor: () => { creates += 1; return supervisor },
      assertNoExternalRuntime: async () => {}, onDaemonState: () => {},
      openMultiUserSwitch: async () => {},
    })

    await expect(controller.resume()).rejects.toThrow(/handoff failed/)
    await controller.resume()
    await controller.cancel()

    expect(creates).toBe(1)
    expect(supervisor.starts).toBe(2)
    expect(supervisor.stops).toBe(1)
  })

  it('uses the selected free port and resumes the same setup job', async () => {
    const supervisor = new FakeSupervisor()
    supervisor.start = async () => {
      supervisor.starts += 1
      if (supervisor.starts === 1) {
        supervisor.state = { kind: 'taken-port', port: 4400, holder: 'another process', suggested: 4401 }
        supervisor.emit('state', supervisor.state)
      }
    }
    let opened = ''
    const controller = new ClientController({
      backUp: async () => {},
      install: async () => '/installed/pagis', beginLaunch: () => {},
      connect: async () => '',
      activate: () => {}, openProduct: async (url) => { opened = url },
      createSupervisor: () => supervisor, assertNoExternalRuntime: async () => {}, onDaemonState: () => {},
      openMultiUserSwitch: async () => {},
    })
    await expect(controller.resume()).rejects.toThrow(/uses port 4400/)

    await controller.usePortAndResume(4401)

    expect(supervisor.state).toMatchObject({ kind: 'running', port: 4401 })
    expect(opened).toContain(':4401/')
  })

  it('ends a first setup on a taken port and keeps the taken-port state for the page', async () => {
    const supervisor = new FakeSupervisor()
    supervisor.start = async () => {
      supervisor.state = { kind: 'taken-port', port: 4400, holder: 'Python (pid 82674)', suggested: 4401 }
      supervisor.emit('state', supervisor.state)
    }
    const states: string[] = []
    let opened = false
    const controller = new ClientController({
      backUp: async () => {},
      install: async () => '/installed/pagis', beginLaunch: () => {}, connect: async () => '',
      activate: () => {}, openProduct: async () => { opened = true },
      createSupervisor: () => supervisor, assertNoExternalRuntime: async () => {},
      onDaemonState: (state) => states.push(state.kind),
      openMultiUserSwitch: async () => {},
    })

    await expect(controller.run({ kind: 'local', people: 'one' })).rejects.toThrow(/uses port 4400/)

    expect(controller.state).toEqual({ kind: 'taken-port', port: 4400, holder: 'Python (pid 82674)', suggested: 4401 })
    expect(states).toEqual(['taken-port'])
    expect(opened).toBe(false)
  })

  it('resumes a setup for several People on the selected port', async () => {
    const supervisor = new FakeSupervisor()
    supervisor.start = async () => {
      supervisor.starts += 1
      if (supervisor.starts === 1) {
        supervisor.state = { kind: 'taken-port', port: 4400, holder: 'another process', suggested: 4401 }
        supervisor.emit('state', supervisor.state)
      }
    }
    const calls: string[] = []
    const controller = new ClientController({
      backUp: async () => {},
      install: async () => '/installed/pagis', beginLaunch: () => {},
      connect: async () => '',
      activate: () => {}, openProduct: async (url) => { calls.push(`open-product:${url}`) },
      createSupervisor: () => supervisor, assertNoExternalRuntime: async () => {}, onDaemonState: () => {},
      openMultiUserSwitch: async () => { calls.push('open-multi-user-switch') },
    })
    await expect(controller.run({ kind: 'local', people: 'several' })).rejects.toThrow(/uses port 4400/)

    await controller.usePortAndResume(4401)

    expect(calls).toEqual(['open-product:http://127.0.0.1:4401/', 'open-multi-user-switch'])
  })

  it('opens the product alone when an installed client starts', async () => {
    const calls: string[] = []
    const controller = new ClientController({
      backUp: async () => {},
      install: async () => '/installed/pagis', beginLaunch: () => {},
      connect: async () => '',
      activate: () => {}, openProduct: async () => { calls.push('open-product') },
      createSupervisor: () => new FakeSupervisor(), assertNoExternalRuntime: async () => {}, onDaemonState: () => {},
      openMultiUserSwitch: async () => { calls.push('open-multi-user-switch') },
    })

    await controller.resume()

    expect(calls).toEqual(['open-product'])
  })

  it('upgrades with a Backup first, then starts the new release and opens the product', async () => {
    const calls: string[] = []
    const controller = new ClientController({
      backUp: async () => { calls.push('back-up') },
      install: async () => { calls.push('install'); return '/installed/pagis' },
      beginLaunch: () => calls.push('begin-launch'),
      connect: async () => '',
      activate: () => calls.push('activate'), openProduct: async () => { calls.push('open-product') },
      createSupervisor: (_binary, beforeSpawn) => {
        const supervisor = new FakeSupervisor()
        const start = supervisor.start.bind(supervisor)
        supervisor.start = async () => { beforeSpawn(); await start() }
        return supervisor
      },
      assertNoExternalRuntime: async () => {}, onDaemonState: () => {},
      openMultiUserSwitch: async () => { calls.push('open-multi-user-switch') },
    })

    await controller.upgrade(true)

    expect(calls).toEqual(['back-up', 'install', 'begin-launch', 'activate', 'open-product'])
  })

  /** Quit cancels the setup job, and the cancel stops the program that
   *  takes the Backup. */
  it('stops the Backup of an Upgrade on a cancel, and starts nothing', async () => {
    let created = 0
    const controller = new ClientController({
      backUp: (signal) => new Promise((_resolve, reject) => {
        signal.addEventListener('abort', () => reject(new DOMException('cancelled', 'AbortError')))
      }),
      install: async () => '/installed/pagis', beginLaunch: () => {}, connect: async () => '',
      activate: () => {}, openProduct: async () => {},
      createSupervisor: () => { created += 1; return new FakeSupervisor() },
      assertNoExternalRuntime: async () => {}, onDaemonState: () => {},
      openMultiUserSwitch: async () => {},
    })
    const upgrade = controller.upgrade(true)
    expect(controller.inProgress).toBe(true)

    await controller.cancel()

    await expect(upgrade).rejects.toThrow(/cancelled/)
    expect(created).toBe(0)
  })

  /** The connect-only path installs nothing and supervises nothing:
   *  no package, no supervisor and no release marker. */
  it('connects to a server without installing or supervising one', async () => {
    const calls: string[] = []
    const controller = new ClientController({
      backUp: async () => {},
      install: async () => { calls.push('install'); return '/installed/pagis' },
      beginLaunch: () => calls.push('begin-launch'),
      connect: async (url) => { calls.push(`connect:${url}`); return 'https://pagis.example.com/' },
      activate: () => calls.push('activate'),
      openProduct: async (url) => { calls.push(`open-product:${url}`) },
      createSupervisor: () => { calls.push('create-supervisor'); return new FakeSupervisor() },
      assertNoExternalRuntime: async () => {}, onDaemonState: () => {},
      openMultiUserSwitch: async () => {},
    })

    await controller.run({ kind: 'server', url: 'pagis.example.com' })

    expect(calls).toEqual([
      'connect:pagis.example.com',
      'open-product:https://pagis.example.com/',
    ])
    expect(controller.state).toBeNull()
  })

  it('does not resume setup after Quit cancels a delayed port change', async () => {
    const supervisor = new FakeSupervisor()
    let finishPort!: () => void
    supervisor.usePort = async () => new Promise<void>((resolve) => { finishPort = resolve })
    let installs = 0
    const controller = new ClientController({
      backUp: async () => {},
      install: async () => { installs += 1; return '/installed/pagis' },
      beginLaunch: () => {}, connect: async () => '', activate: () => {}, openProduct: async () => {},
      createSupervisor: () => supervisor, assertNoExternalRuntime: async () => {}, onDaemonState: () => {},
      openMultiUserSwitch: async () => {},
    })
    await controller.resume()
    const changing = controller.usePortAndResume(4401)
    while (!finishPort) await Promise.resolve()

    await controller.cancel()
    finishPort()

    await expect(changing).rejects.toThrow(/cancelled/)
    expect(installs).toBe(1)
    expect(supervisor.stops).toBe(1)
  })

  /** Quit asks for a confirmation only while an installation or a
   *  start-up of the local server runs. A check of a server address, a
   *  failure and a running server are not in progress. */
  it('is in progress only while it installs or starts the local server', async () => {
    const supervisor = new FakeSupervisor()
    let finishInstall!: () => void
    let finishConnect!: () => void
    const controller = new ClientController({
      backUp: async () => {},
      install: () => new Promise((resolve) => { finishInstall = () => resolve('/installed/pagis') }),
      beginLaunch: () => {},
      connect: () => new Promise((resolve) => { finishConnect = () => resolve('https://pagis.example.com/') }),
      activate: () => {}, openProduct: async () => {},
      createSupervisor: () => supervisor, assertNoExternalRuntime: async () => {}, onDaemonState: () => {},
      openMultiUserSwitch: async () => {},
    })
    expect(controller.inProgress).toBe(false)

    const connecting = controller.run({ kind: 'server', url: 'pagis.example.com' })
    expect(controller.inProgress).toBe(false)
    finishConnect()
    await connecting

    const installing = controller.run({ kind: 'local', people: 'one' })
    expect(controller.inProgress).toBe(true)
    finishInstall()
    await installing
    expect(controller.inProgress).toBe(false)

    // The supervisor starts the server again after a crash.
    supervisor.state = { kind: 'starting' }
    expect(controller.inProgress).toBe(true)
    supervisor.state = { kind: 'failed', reason: 'The daemon stopped 4 times with code 1.', log: '' }
    expect(controller.inProgress).toBe(false)
  })
})
