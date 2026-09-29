import type { DaemonState } from './daemon'
import type { InstallOptions } from './runtimeInstaller'
import { SetupCoordinator, setupRequest, type SetupRequest } from './setupCoordinator'

export interface ClientSupervisor {
  readonly state: DaemonState
  start(): Promise<void>
  stop(): Promise<void>
  usePort(port: number): Promise<void>
  on(event: 'state', listener: (state: DaemonState) => void): unknown
}

export interface ClientControllerDependencies {
  install(options: InstallOptions): Promise<string>
  beginLaunch(): void
  activate(): void
  /** Check a server this client did not start at the address that the
   *  person typed, and answer its origin. It installs and supervises
   *  nothing. */
  connect(url: string, signal?: AbortSignal): Promise<string>
  openProduct(url: string): Promise<void>
  openMultiUserSwitch(): Promise<void>
  createSupervisor(binary: string, beforeSpawn: () => void): ClientSupervisor
  assertNoExternalRuntime(): Promise<void>
  onDaemonState(state: DaemonState): void
}

/** A start of an installed client runs its server and opens the product.
 *  The setup question belongs to setup alone, so a start asks nothing more. */
const RESUME: SetupRequest = { kind: 'local', people: 'one' }

export class ClientController {
  private supervisor: ClientSupervisor | null = null
  private abort: AbortController | null = null
  private intent = 0
  // The request of the setup job a port change resumes. It lives only
  // as long as this process, and nothing writes it down.
  private request: SetupRequest = RESUME
  private readonly setup: SetupCoordinator

  constructor(private readonly dependencies: ClientControllerDependencies) {
    this.setup = new SetupCoordinator({
      install: async () => {
        const abort = new AbortController()
        this.abort = abort
        return this.dependencies.install({
          signal: abort.signal,
          beforeReplace: () => this.releaseRuntimeForRepair(),
        })
      },
      cancel: async () => {
        this.abort?.abort()
        await this.supervisor?.stop()
      },
      start: (binary) => this.startServer(binary),
      activate: () => this.dependencies.activate(),
      openProduct: (url) => this.dependencies.openProduct(url),
      connect: (url) => {
        const abort = new AbortController()
        this.abort = abort
        return this.dependencies.connect(url, abort.signal)
      },
      openMultiUserSwitch: () => this.dependencies.openMultiUserSwitch(),
    })
  }

  get state(): DaemonState | null { return this.supervisor?.state ?? null }

  /** Whether an installation or a start-up of the local server runs. A
   *  check of a server address is neither. */
  get inProgress(): boolean {
    return this.setup.setsUpLocal || this.supervisor?.state.kind === 'starting'
  }

  /** Start the installed server and open the product. */
  resume(): Promise<void> { return this.run(RESUME) }

  /** Run the setup the setup page asked for. The request is not trusted. */
  run(request: unknown): Promise<void> {
    this.request = setupRequest(request) ?? this.request
    const job = this.setup.run(request)
    const operationAbort = this.abort
    void job.finally(() => {
      if (this.abort === operationAbort) this.abort = null
    }).catch(() => undefined)
    return job
  }

  cancel(): Promise<void> {
    this.intent += 1
    return this.setup.cancel()
  }

  retry(): Promise<void> | undefined { return this.supervisor?.start() }

  async usePortAndResume(port: number): Promise<void> {
    if (!this.supervisor) throw new Error('the Pagis server has not started')
    const intent = ++this.intent
    await this.supervisor.usePort(port)
    if (intent !== this.intent) throw new Error('Pagis setup was cancelled')
    await this.run(this.request)
  }

  private async startServer(binary: string): Promise<string> {
    if (!this.supervisor) {
      this.supervisor = this.dependencies.createSupervisor(binary, () => this.dependencies.beginLaunch())
      this.supervisor.on('state', (state) => this.dependencies.onDaemonState(state))
    }
    await this.supervisor.start()
    const state = this.supervisor.state
    if (state.kind === 'running') return state.url
    if (state.kind === 'taken-port') throw new Error(`Another process uses port ${state.port}.`)
    throw new Error(state.kind === 'failed' ? state.reason : 'the Pagis server did not start')
  }

  private async releaseRuntimeForRepair(): Promise<void> {
    const state = this.supervisor?.state
    if (state?.kind === 'running') {
      if (!state.owned) throw new Error('Another Pagis client owns the running server. Quit it, then select Retry.')
      await this.supervisor?.stop()
    }
    await this.dependencies.assertNoExternalRuntime()
  }
}
