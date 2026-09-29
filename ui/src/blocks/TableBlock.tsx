// The table block (ADR-0004): a declarative grid after Adaptive
// Cards' `ColumnSet`. No server pagination and no sort configuration —
// the client sorts locally, and past 100 rows the agent posts a CSV
// artifact as a `file` block instead. A cell is a closed union, so a
// cell carries no markup and nothing here renders model text as HTML.

import { useMemo, useState } from 'react'
import { ChevronDown, ChevronUp } from 'lucide-react'

import type { components } from '../api/schema'
import { Button } from '../primitives'
import { Card } from './Card'

import './TableBlock.css'

type KnownBlock = components['schemas']['KnownBlock']
type TableBlockType = Extract<KnownBlock, { type: 'table' }>
type TableCell = components['schemas']['TableCell']

type Direction = 'asc' | 'desc'
type Sort = { column: number; direction: Direction }

/** The sort key of one cell: its own value, not its rendering. */
function sortKey(cell: TableCell | undefined): string | number {
  if (cell === undefined) return ''
  switch (cell.kind) {
    case 'text':
      return cell.text
    case 'number':
      return cell.number
    case 'timestamp':
      return cell.unix_ms
    case 'link':
      return cell.label ?? cell.href
  }
}

function compare(left: TableCell | undefined, right: TableCell | undefined) {
  const a = sortKey(left)
  const b = sortKey(right)
  if (typeof a === 'number' && typeof b === 'number') return a - b
  // A mixed column falls back to text, so a sort never throws away rows.
  return String(a).localeCompare(String(b))
}

/** `YYYY-MM-DD HH:MM` in the reader's own zone. */
function formatTimestamp(unixMs: number): string {
  const at = new Date(unixMs)
  if (Number.isNaN(at.getTime())) return String(unixMs)
  return at.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

function CellView({ cell }: { cell: TableCell | undefined }) {
  if (cell === undefined) return null
  switch (cell.kind) {
    case 'text':
      return <>{cell.text}</>
    case 'number':
      return <>{cell.number}</>
    case 'timestamp':
      return <time dateTime={new Date(cell.unix_ms).toISOString()}>
        {formatTimestamp(cell.unix_ms)}
      </time>
    case 'link':
      return (
        <a href={cell.href} target="_blank" rel="noreferrer noopener">
          {cell.label ?? cell.href}
        </a>
      )
  }
}

export function TableBlock({ block }: { block: TableBlockType }) {
  const [sort, setSort] = useState<Sort | null>(null)
  const columns = block.columns
  const rows = useMemo(() => block.rows ?? [], [block.rows])

  const ordered = useMemo(() => {
    if (sort === null) return rows
    const sorted = [...rows].sort((left, right) =>
      compare(left[sort.column], right[sort.column]),
    )
    return sort.direction === 'asc' ? sorted : sorted.reverse()
  }, [rows, sort])

  // One header click cycles ascending, descending, then back to the
  // order the agent wrote, which is itself information.
  const toggle = (column: number) =>
    setSort((current) => {
      if (current === null || current.column !== column) {
        return { column, direction: 'asc' }
      }
      return current.direction === 'asc'
        ? { column, direction: 'desc' }
        : null
    })

  return (
    <Card data-testid="table-block">
      <div className="block-table">
        <table>
          <thead>
            <tr>
              {columns.map((column, index) => (
                <th
                  key={column.key}
                  scope="col"
                  className={`align-${column.align ?? 'left'}`}
                  aria-sort={
                    sort?.column === index
                      ? sort.direction === 'asc'
                        ? 'ascending'
                        : 'descending'
                      : 'none'
                  }
                >
                  <Button size="sm" onClick={() => toggle(index)}>
                    {column.label}
                    <span className="block-table-sort" aria-hidden="true">
                      {sort?.column === index ? (
                        sort.direction === 'asc' ? (
                          <ChevronUp size={14} aria-hidden />
                        ) : (
                          <ChevronDown size={14} aria-hidden />
                        )
                      ) : null}
                    </span>
                  </Button>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {ordered.map((row, index) => (
              <tr key={index}>
                {columns.map((column, cell) => (
                  <td
                    key={column.key}
                    className={`align-${column.align ?? 'left'}`}
                  >
                    <CellView cell={row[cell]} />
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Card>
  )
}
