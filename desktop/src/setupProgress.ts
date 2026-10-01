import type { InstallProgress } from './runtimeInstaller'
import type { SetupState, SetupStep } from './setupState'

/** The span of the speed that gives the time left. A shorter span makes
 *  the estimate jump with each change of speed. */
const SPEED_WINDOW_MS = 5000
/** The first part of a download gives no estimate: the speed of the
 *  first second says little about the rest. */
const FIRST_ESTIMATE_MS = 2000
/** The setup page gets at most four download updates a second. */
const SHOW_INTERVAL_MS = 250

const STEP_OF_PHASE: Record<InstallProgress['phase'], SetupStep> = {
  downloading: 'download',
  verifying: 'check',
  extracting: 'install',
  activating: 'install',
}

/**
 * Turn the phases of the Runtime installer and the start of the server
 * into the steps that the setup page shows. A download also gets its
 * share and its time left, from the speed of its last five seconds, as
 * the download windows of browsers and of the Finder do.
 */
export class SetupProgress {
  private samples: { at: number; received: number }[] = []
  private firstAt = 0
  private shownAt = Number.NEGATIVE_INFINITY

  constructor(
    private readonly show: (state: SetupState) => void,
    private readonly now: () => number = Date.now,
  ) {}

  report(progress: InstallProgress): void {
    if (progress.phase !== 'downloading') {
      this.samples = []
      this.show({ kind: 'setting-up', step: STEP_OF_PHASE[progress.phase], download: null })
      return
    }
    const at = this.now()
    if (this.samples.length === 0) this.firstAt = at
    this.samples.push({ at, received: progress.received })
    while (this.samples.length > 2 && this.samples[1].at <= at - SPEED_WINDOW_MS) this.samples.shift()
    const last = progress.received === progress.total
    if (this.samples.length > 1 && !last && at - this.shownAt < SHOW_INTERVAL_MS) return
    this.shownAt = at
    this.show({
      kind: 'setting-up',
      step: 'download',
      download: {
        fraction: progress.total === 0 ? 1 : progress.received / progress.total,
        detail: downloadDetail(progress.received, progress.total, this.secondsLeft(at, progress)),
      },
    })
  }

  /** The server is installed, and the client starts it. */
  starting(): void {
    this.show({ kind: 'setting-up', step: 'start', download: null })
  }

  private secondsLeft(at: number, progress: { received: number; total: number }): number | null {
    if (at - this.firstAt < FIRST_ESTIMATE_MS) return null
    const first = this.samples[0]
    const elapsed = at - first.at
    const speed = elapsed > 0 ? (progress.received - first.received) / elapsed : 0
    if (speed <= 0) return null
    return (progress.total - progress.received) / speed / 1000
  }
}

/** The megabytes received of the whole, and the time left when it is
 *  known: "31 of 72 MB · About 20 seconds left". */
export function downloadDetail(received: number, total: number, secondsLeft: number | null): string {
  const megabytes = `${Math.round(received / 1_000_000)} of ${Math.round(total / 1_000_000)} MB`
  return secondsLeft === null ? megabytes : `${megabytes} · ${timeLeft(secondsLeft)}`
}

/** The time left in round words. Seconds go up to the next five, so the
 *  estimate does not tick down each second; minutes are whole. */
export function timeLeft(seconds: number): string {
  if (seconds <= 5) return 'A few seconds left'
  const rounded = Math.ceil(seconds / 5) * 5
  if (rounded < 60) return `About ${rounded} seconds left`
  const minutes = Math.max(1, Math.round(seconds / 60))
  return minutes === 1 ? 'About 1 minute left' : `About ${minutes} minutes left`
}
