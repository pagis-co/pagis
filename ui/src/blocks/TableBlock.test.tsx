// The table block: declarative, non-interactive except for a
// local sort, and every cell kind renders from its own value. A
// 100-row table is the most the daemon sends: past that the agent
// posts a CSV artifact as a `file` block instead.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, MessageDto } from '../api/client'
import { Blocks } from './BlockView'

const api = { GET: vi.fn(async () => ({ data: undefined })) }

function mount(blocks: unknown) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <Blocks
        blocks={blocks as MessageDto['blocks']}
        api={api as unknown as ApiClient}
      />
    </QueryClientProvider>,
  )
}

function bodyColumn(column: number): string[] {
  return Array.from(
    screen.getByTestId('table-block').querySelectorAll('tbody tr'),
  ).map((row) => row.querySelectorAll('td')[column]!.textContent!.trim())
}

const table = {
  type: 'table',
  columns: [
    { key: 'name', label: 'Name' },
    { key: 'runs', label: 'Runs', align: 'right' },
  ],
  rows: [
    [
      { kind: 'text', text: 'nightly' },
      { kind: 'number', number: 9 },
    ],
    [
      { kind: 'text', text: 'ad hoc' },
      { kind: 'number', number: 11 },
    ],
    [
      { kind: 'text', text: 'weekly' },
      { kind: 'number', number: 2 },
    ],
  ],
}

describe('TableBlock', () => {
  it('renders the grid in the order the agent wrote', () => {
    mount([table])

    expect(screen.getByText('Name')).toBeTruthy()
    expect(bodyColumn(0)).toEqual(['nightly', 'ad hoc', 'weekly'])
  })

  it('sorts locally, ascending then descending then back', () => {
    mount([table])
    const runs = screen.getByRole('button', { name: /Runs/ })

    // A number column sorts numerically, not as text: 2 before 9.
    fireEvent.click(runs)
    expect(bodyColumn(1)).toEqual(['2', '9', '11'])
    fireEvent.click(runs)
    expect(bodyColumn(1)).toEqual(['11', '9', '2'])
    fireEvent.click(runs)
    expect(bodyColumn(0)).toEqual(['nightly', 'ad hoc', 'weekly'])
  })

  it('names the sorted column for a screen reader', () => {
    mount([table])
    fireEvent.click(screen.getByRole('button', { name: /Name/ }))

    const headers = screen.getByTestId('table-block').querySelectorAll('th')
    expect(headers[0]!.getAttribute('aria-sort')).toBe('ascending')
    expect(headers[1]!.getAttribute('aria-sort')).toBe('none')
  })

  it('renders every cell kind from its own value', () => {
    mount([
      {
        type: 'table',
        columns: [
          { key: 'a', label: 'A' },
          { key: 'b', label: 'B' },
          { key: 'c', label: 'C' },
          { key: 'd', label: 'D' },
        ],
        rows: [
          [
            { kind: 'text', text: 'plain' },
            { kind: 'number', number: 3.5 },
            { kind: 'link', href: 'https://example.com', label: 'here' },
            { kind: 'timestamp', unix_ms: 0 },
          ],
        ],
      },
    ])

    expect(screen.getByText('plain')).toBeTruthy()
    expect(screen.getByText('3.5')).toBeTruthy()
    const link = screen.getByRole('link', { name: 'here' })
    expect(link.getAttribute('href')).toBe('https://example.com')
    expect(link.getAttribute('rel')).toContain('noopener')
    // A timestamp reads as a date, and carries the instant for a reader.
    expect(
      screen
        .getByTestId('table-block')
        .querySelector('time')!
        .getAttribute('dateTime'),
    ).toBe('1970-01-01T00:00:00.000Z')
  })

  it('renders a hundred rows, the most the daemon sends', () => {
    mount([
      {
        type: 'table',
        columns: [{ key: 'r', label: 'R' }],
        rows: Array.from({ length: 100 }, (_unused, index) => [
          { kind: 'text', text: `row ${index}` },
        ]),
      },
    ])

    expect(
      screen.getByTestId('table-block').querySelectorAll('tbody tr'),
    ).toHaveLength(100)
    expect(screen.getByText('row 99')).toBeTruthy()
  })

  it('renders a table with no rows as its header alone', () => {
    mount([{ type: 'table', columns: [{ key: 'r', label: 'R' }] }])

    expect(screen.getByText('R')).toBeTruthy()
    expect(
      screen.getByTestId('table-block').querySelectorAll('tbody tr'),
    ).toHaveLength(0)
  })

  it('falls back when the block carries no columns', () => {
    mount([{ type: 'table', rows: [] }])
    expect(screen.getByTestId('unknown-block')).toBeTruthy()
  })
})
