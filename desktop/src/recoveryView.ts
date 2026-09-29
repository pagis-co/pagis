export type RecoveryState = { kind: 'starting' | 'running' | 'taken-port' | 'failed' }
export type RecoveryView = 'setup' | 'product' | 'status'

export function recoveryView(state: RecoveryState, setupOpen: boolean, recoveryWindowOpen: boolean): RecoveryView {
  if (setupOpen) return 'setup'
  if (state.kind === 'running') return recoveryWindowOpen ? 'product' : 'setup'
  return 'status'
}

export function sameProductOrigin(expected: string, target: string): boolean {
  try { return new URL(target).origin === expected } catch { return false }
}
