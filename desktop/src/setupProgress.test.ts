// The steps of a local setup, as the setup page shows them, and the
// progress and the time left of the server download.

import { describe, expect, it } from 'vitest'

import { SetupProgress, downloadDetail, timeLeft } from './setupProgress'
import type { SetupState } from './setupState'

const MB = 1_000_000

/** A setup progress on a clock that the test moves. */
function progress(): { shown: SetupState[]; at(ms: number): void; setup: SetupProgress } {
  const shown: SetupState[] = []
  let now = 0
  return {
    shown,
    at: (ms) => { now = ms },
    setup: new SetupProgress((state) => shown.push(state), () => now),
  }
}

describe('the time left of the server download', () => {
  it.each([
    [1, 'A few seconds left'],
    [5, 'A few seconds left'],
    [5.2, 'About 10 seconds left'],
    [21, 'About 25 seconds left'],
    [54, 'About 55 seconds left'],
    [56, 'About 1 minute left'],
    [100, 'About 2 minutes left'],
    [3600, 'About 60 minutes left'],
  ])('says %s seconds as "%s"', (seconds, words) => {
    expect(timeLeft(seconds)).toBe(words)
  })
})

describe('the detail of the server download', () => {
  it('gives the megabytes received of the whole, and the time left when it is known', () => {
    expect(downloadDetail(31.4 * MB, 71.7 * MB, null)).toBe('31 of 72 MB')
    expect(downloadDetail(31.4 * MB, 71.7 * MB, 20)).toBe('31 of 72 MB · About 20 seconds left')
  })
})

describe('the progress of a local setup', () => {
  it('names the step of each installer phase, and the start', () => {
    const { shown, setup } = progress()

    setup.report({ phase: 'verifying' })
    setup.report({ phase: 'extracting' })
    setup.report({ phase: 'activating' })
    setup.starting()

    expect(shown).toEqual([
      { kind: 'setting-up', step: 'check', download: null },
      { kind: 'setting-up', step: 'install', download: null },
      { kind: 'setting-up', step: 'install', download: null },
      { kind: 'setting-up', step: 'start', download: null },
    ])
  })

  it('shows the share of the download, and no time left in its first two seconds', () => {
    const { shown, at, setup } = progress()

    setup.report({ phase: 'downloading', received: 0, total: 72 * MB })
    at(1000)
    setup.report({ phase: 'downloading', received: 9 * MB, total: 72 * MB })

    expect(shown).toEqual([
      { kind: 'setting-up', step: 'download', download: { fraction: 0, detail: '0 of 72 MB' } },
      { kind: 'setting-up', step: 'download', download: { fraction: 0.125, detail: '9 of 72 MB' } },
    ])
  })

  it('estimates the time left from the speed of the last five seconds', () => {
    const { shown, at, setup } = progress()
    // One megabyte a second for ten seconds, then four.
    for (let second = 0; second <= 10; second += 1) {
      at(second * 1000)
      setup.report({ phase: 'downloading', received: second * MB, total: 72 * MB })
    }
    for (let second = 11; second <= 15; second += 1) {
      at(second * 1000)
      setup.report({ phase: 'downloading', received: (10 + (second - 10) * 4) * MB, total: 72 * MB })
    }

    // 42 MB to go at four megabytes a second.
    expect(shown.at(-1)).toEqual({
      kind: 'setting-up',
      step: 'download',
      download: { fraction: 30 / 72, detail: '30 of 72 MB · About 15 seconds left' },
    })
  })

  it('shows the download at most four times a second, and always its last byte', () => {
    const { shown, at, setup } = progress()

    setup.report({ phase: 'downloading', received: 0, total: 72 * MB })
    at(100)
    setup.report({ phase: 'downloading', received: 1 * MB, total: 72 * MB })
    at(200)
    setup.report({ phase: 'downloading', received: 72 * MB, total: 72 * MB })
    at(300)
    setup.report({ phase: 'verifying' })

    expect(shown.map((state) => state.kind === 'setting-up' ? state.download?.fraction ?? state.step : null))
      .toEqual([0, 1, 'check'])
  })
})
