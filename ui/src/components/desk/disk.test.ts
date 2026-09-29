// The disk figure of the Desk Panel footer, written as `disk 3.1 GB`.

import { describe, expect, it } from 'vitest'

import { diskFigure } from './disk'

describe('diskFigure', () => {
  it('writes gigabytes with one decimal', () => {
    expect(diskFigure(3_100_000_000)).toBe('disk 3.1 GB')
  })

  it('writes a small disk in megabytes', () => {
    expect(diskFigure(412_000_000)).toBe('disk 412 MB')
  })

  it('writes an empty disk as zero, not as nothing', () => {
    expect(diskFigure(0)).toBe('disk 0 MB')
  })

  it('says nothing when Docker could not say', () => {
    expect(diskFigure(null)).toBeNull()
    expect(diskFigure(undefined)).toBeNull()
  })
})
