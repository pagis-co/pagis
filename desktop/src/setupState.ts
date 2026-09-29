import type { DaemonState } from './daemon'

/** What the setup page shows. */
export type SetupState =
  | { kind: 'ready' }
  | { kind: 'installing'; detail: string }
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
