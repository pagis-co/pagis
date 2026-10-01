import type { DaemonState } from './daemon'

/** A step of the setup of the local server, in the order that the setup
 *  page lists them. */
export type SetupStep = 'download' | 'check' | 'install' | 'start'

/** The share of the Server Package that the client has received, from 0
 *  to 1, and the words under the bar: the megabytes and the time left. */
export interface DownloadProgress {
  fraction: number
  detail: string
}

/** What the setup page shows. */
export type SetupState =
  | { kind: 'ready' }
  /** A short task with no steps: a check of a server address, or a start
   *  on another port. */
  | { kind: 'installing'; detail: string }
  /** The setup of the local server, at one of its steps. `download` is
   *  set only while the client downloads the Server Package. */
  | { kind: 'setting-up'; step: SetupStep; download: DownloadProgress | null }
  /** `repair` is true where this computer holds an installation that a
   *  repair can check and start again. A repair asks no setup question. */
  | { kind: 'failed'; reason: string; repair: boolean }
  | { kind: 'taken-port'; port: number; holder: string; suggested: number }
  /** A client connected to a server could not open it. It installed
   *  nothing on this computer, so there is nothing to repair. */
  | { kind: 'connection-failed'; origin: string; reason: string }
  /** The checks of the server address that the Person typed failed: no
   *  server answered, its release is outside the Compatibility Range, or
   *  it has no administrator yet. The page shows the reason under the
   *  Server address field, and the Person corrects the address there. */
  | { kind: 'server-check-failed'; reason: string }

/**
 * The state of the setup page after a setup or a start fails. A taken
 * port is not a failure of the server: the page names the process that
 * holds the port and the next free port, and offers that port.
 */
export function setupFailureState(reason: string, daemon: DaemonState | null, installed: boolean): SetupState {
  if (daemon?.kind === 'taken-port') return daemon
  return { kind: 'failed', reason, repair: installed }
}
