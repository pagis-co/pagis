// What the setup page shows when a setup or a start fails (ADR-0025).

import { describe, expect, it } from 'vitest'

import { setupFailureState } from './setupState'

describe('the setup page after a failure', () => {
  /** The first start of a new installation finds its port taken. The
   *  page names the process and the next free port, and does not say
   *  that the server did not start. */
  it('shows a taken port at the first start with the holder and the next free port', () => {
    const state = setupFailureState(
      'the server stopped',
      { kind: 'taken-port', port: 4410, holder: 'Python (pid 82674)', suggested: 4411 },
      false,
    )

    expect(state).toEqual({ kind: 'taken-port', port: 4410, holder: 'Python (pid 82674)', suggested: 4411 })
  })

  it('offers Repair only where this computer holds an installation', () => {
    expect(setupFailureState('the download stopped', null, false))
      .toEqual({ kind: 'failed', reason: 'the download stopped', repair: false, log: '' })
    expect(setupFailureState('the server stopped 4 times', { kind: 'failed', reason: 'x', log: '' }, true))
      .toEqual({ kind: 'failed', reason: 'the server stopped 4 times', repair: true, log: '' })
  })

  /** The reason says only that the server stopped. What the server
   *  printed says why, so the page shows it too. */
  it('keeps what the stopped server printed', () => {
    const log = 'Error: migration 2 was previously applied but is missing in the resolved migrations'

    expect(setupFailureState('The daemon stopped 4 times with code 1.', { kind: 'failed', reason: 'x', log }, true))
      .toEqual({ kind: 'failed', reason: 'The daemon stopped 4 times with code 1.', repair: true, log })
  })
})
