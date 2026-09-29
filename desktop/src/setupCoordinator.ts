/**
 * Who uses a local installation, as the setup page asks it. The answer
 * only chooses where setup goes next: nothing stores it, and the
 * Multi-User Mode stays derived from the Public Origin.
 */
export type LocalPeople = 'one' | 'several'

/**
 * What the setup page asked for. The local installation is
 * installed and supervised by this client; the server one is the
 * address of the server alone, and the client starts nothing. The
 * Person signs in on that server's own page.
 */
export type SetupRequest =
  | { kind: 'local'; people: LocalPeople }
  | { kind: 'server'; url: string }

export interface SetupDependencies {
  install(): Promise<string>
  cancel?(): void | Promise<void>
  start(binary: string): Promise<string>
  activate(): void
  openProduct(url: string): Promise<void>
  /** Check the server at the address that the person typed, and answer
   *  its origin. */
  connect(url: string): Promise<string>
  /** Open the Administration Interface, signed in, on the Multi-User
   *  Mode switch of the installation this client started. */
  openMultiUserSwitch(): Promise<void>
}

export class SetupCoordinator {
  private job: Promise<void> | null = null
  private jobKind: SetupRequest['kind'] | null = null
  private generation = 0

  constructor(private readonly dependencies: SetupDependencies) {}

  /** Whether a setup of the local installation runs: its install, its
   *  start or its handoff. */
  get setsUpLocal(): boolean { return this.jobKind === 'local' }

  run(request: unknown): Promise<void> {
    const parsed = setupRequest(request)
    if (!parsed) return Promise.reject(refusal(request))
    if (this.job) return this.job
    const generation = ++this.generation
    const job = parsed.kind === 'local'
      ? this.runLocal(parsed.people, generation)
      : this.runServer(parsed.url, generation)
    this.jobKind = parsed.kind
    this.job = job.finally(() => { this.job = null; this.jobKind = null })
    return this.job
  }

  async cancel(): Promise<void> {
    const job = this.job
    this.generation += 1
    await this.dependencies.cancel?.()
    await job?.catch(() => undefined)
  }

  /**
   * Install the one locked server release, start it and hand over. For
   * several People, the owner then turns on the Multi-User Mode in the
   * Administration Interface.
   */
  private async runLocal(people: LocalPeople, generation: number): Promise<void> {
    const binary = await this.dependencies.install()
    this.requireCurrent(generation)
    const url = await this.dependencies.start(binary)
    this.requireCurrent(generation)
    this.dependencies.activate()
    this.requireCurrent(generation)
    await this.dependencies.openProduct(url)
    if (people === 'one') return
    this.requireCurrent(generation)
    await this.dependencies.openMultiUserSwitch()
  }

  /** Check a server that is already running, and hand over to its own
   *  sign-in page. */
  private async runServer(url: string, generation: number): Promise<void> {
    const origin = await this.dependencies.connect(url)
    this.requireCurrent(generation)
    await this.dependencies.openProduct(origin)
  }

  private requireCurrent(generation: number): void {
    if (generation !== this.generation) throw new Error('Pagis setup was cancelled')
  }
}

/**
 * Why the main process refuses a setup request, in words for a person.
 * The setup page checks each detail before it sends it, so only a page
 * that skips its checks gets this refusal. The setup page shows it as
 * the reason, so it names what the person does next.
 */
function refusal(request: unknown): Error {
  return new Error(isServerRequest(request)
    ? 'Enter the address of your Pagis server, then select Continue.'
    : 'Pagis does not know this setup. Choose a setup again.')
}

/** Whether the setup page asked to connect to a server, valid request
 *  or not. A failure of such a request offers no repair, because the
 *  client installs nothing on this computer for it. */
export function isServerRequest(value: unknown): boolean {
  return value !== null && typeof value === 'object' && (value as { kind?: unknown }).kind === 'server'
}

/**
 * What the setup page sent, or null when it is not a setup request at
 * all. The privileged IPC validates every argument, so the shape is
 * checked here and nowhere else.
 */
export function setupRequest(value: unknown): SetupRequest | null {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return null
  const record = value as Record<string, unknown>
  const keys = Object.keys(record)
  if (record.kind === 'local') {
    if (keys.length !== 2) return null
    return record.people === 'one' || record.people === 'several'
      ? { kind: 'local', people: record.people }
      : null
  }
  if (record.kind !== 'server' || keys.length !== 2) return null
  const { url } = record
  if (typeof url !== 'string' || url.trim().length === 0) return null
  return { kind: 'server', url }
}
