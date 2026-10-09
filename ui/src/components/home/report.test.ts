// The Report and the work record on Home (ADR-0022). The rules
// are pure, so the heading, the brief line and the record read the same
// whatever the page does around them.

import { describe, expect, it } from 'vitest'

import type { RunDto } from '../../api/client'
import { briefLine, homeDate, workRecord } from './report'

const WEDNESDAY = new Date('2026-09-16T16:58:00').getTime()

function run(fields: Partial<RunDto>): RunDto {
  return {
    id: 'run-1',
    agent_id: 'ag-1',
    channel_id: 'ch-1',
    origin_channel_id: null,
    root_message_id: null,
    title: 'Book the Austin trip',
    trigger_kind: 'message',
    trigger_ref: null,
    hop_count: 0,
    state: 'completed',
    error: null,
    started_at: WEDNESDAY,
    ended_at: WEDNESDAY,
    created_at: WEDNESDAY,
    duration_ms: 9000,
    ...fields,
  }
}

describe('homeDate', () => {
  it('names the weekday and the date', () => {
    expect(homeDate(WEDNESDAY)).toBe('Wednesday, September 16')
  })
})

describe('briefLine', () => {
  it('names the Chief of Staff and the time of the Report', () => {
    expect(briefLine('Sage', WEDNESDAY)).toMatch(/^Sage’s brief · /)
  })

  it('says a Report is on its way while one is written', () => {
    expect(briefLine('Sage', null, true)).toBe('Sage is writing your brief')
  })

  it('says there is no Report yet before the first one', () => {
    expect(briefLine('Sage', null)).toBe('Sage has written no brief yet')
  })

  it('speaks of the office when no Agent is the Chief of Staff', () => {
    expect(briefLine(null, null)).toBe('No sprite is the chief of staff')
  })
})

describe('workRecord', () => {
  it('puts today first and keeps the older work under it', () => {
    const older = WEDNESDAY - 3 * 86_400_000
    const rows = workRecord(
      [
        run({ id: 'old', ended_at: older, created_at: older }),
        run({ id: 'new' }),
      ],
      WEDNESDAY,
    )
    expect(rows.map((row) => row.run.id)).toEqual(['new', 'old'])
    expect(rows[0]?.today).toBe(true)
    expect(rows[1]?.today).toBe(false)
  })

  it('stamps today with a clock and an older day with its date', () => {
    const older = WEDNESDAY - 3 * 86_400_000
    const rows = workRecord([run({ id: 'old', ended_at: older, created_at: older })], WEDNESDAY)
    expect(rows[0]?.stamp).not.toMatch(/:/)
    expect(workRecord([run({})], WEDNESDAY)[0]?.stamp).toMatch(/:/)
  })

  it('keeps the runs that finished and drops the ones that did not', () => {
    const rows = workRecord([run({ id: 'failed', state: 'failed' }), run({ id: 'done' })], WEDNESDAY)
    expect(rows.map((row) => row.run.id)).toEqual(['done'])
  })

  it('reads at most one page of the record', () => {
    const many = Array.from({ length: 30 }, (_, index) =>
      run({ id: `run-${index}`, ended_at: WEDNESDAY - index * 1000 }),
    )
    expect(workRecord(many, WEDNESDAY)).toHaveLength(12)
  })
})
