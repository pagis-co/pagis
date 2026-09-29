// The disk figure of the Desk Panel footer: what the office
// keeps on disk.

/** A gigabyte, as a disk is sold and as Docker reports it. */
const GIGABYTE = 1_000_000_000
const MEGABYTE = 1_000_000

/**
 * `disk 3.1 GB`, the line the footer writes. A disk under one gigabyte
 * reads in whole megabytes, because a decimal there says nothing.
 * `null` is a disk Docker could not measure: the footer leaves it out
 * rather than claiming a size.
 */
export function diskFigure(bytes: number | null | undefined): string | null {
  if (bytes === null || bytes === undefined) return null
  if (bytes >= GIGABYTE) return `disk ${(bytes / GIGABYTE).toFixed(1)} GB`
  return `disk ${Math.round(bytes / MEGABYTE)} MB`
}
