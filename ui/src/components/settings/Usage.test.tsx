// The Usage section: the month's total, the cap beside it, and
// one row per run.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../../api/client'
import { Usage } from './Usage'

function body(capUsd: number | null, costUsd: number) {
  return {
    from: 0,
    to: 1,
    total: {
      input_tokens: 1_200,
      output_tokens: 300,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      cost_usd: costUsd,
      calls: 2,
    },
    monthly_spend_cap_usd: capUsd,
    runs: [
      {
        run_id: 'r-1',
        last_at: 1_700_000_000_000,
        total: {
          input_tokens: 1_200,
          output_tokens: 300,
          cache_read_tokens: 0,
          cache_write_tokens: 0,
          cost_usd: costUsd,
          calls: 2,
        },
      },
    ],
  }
}

/** The month total, which the run rows repeat, so a test reads it from
 *  the one element that is the headline. It waits: the element renders
 *  at zero before the read answers. */
async function expectMonthTotal(expected: string) {
  await waitFor(() => {
    const total = screen.getByText((_, element) => element?.className === 'usage-total')
    expect(total.textContent).toBe(expected)
  })
}

function mount(data: ReturnType<typeof body> | { runs: [] }) {
  const api = {
    GET: vi.fn(async () => ({ data })),
  }
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={queryClient}>
      <Usage api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  )
  return api
}

describe('Usage', () => {
  it('shows the month total, the tokens and the run it came from', async () => {
    mount(body(null, 1.25))

    await expectMonthTotal('$1.25')
    expect(screen.getByText('No monthly cap')).toBeTruthy()
    expect(
      screen.getByText(/1,200 tokens in · 300 tokens out · 2 model calls/),
    ).toBeTruthy()
    expect(screen.getByText('1,200 in · 300 out')).toBeTruthy()
    expect(screen.getByText('2 calls')).toBeTruthy()
  })

  it('names the cap and says when it is reached', async () => {
    mount(body(5, 5.5))

    await expectMonthTotal('$5.50')
    expect(screen.getByText('of a $5.00 monthly cap')).toBeTruthy()
    expect(screen.getByText('Cap reached')).toBeTruthy()
  })

  it('says a month with no model call has none', async () => {
    mount({
      from: 0,
      to: 1,
      total: {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_usd: 0,
        calls: 0,
      },
      monthly_spend_cap_usd: null,
      runs: [],
    })

    expect(await screen.findByText('No model calls this month.')).toBeTruthy()
    await expectMonthTotal('$0.00')
  })

  /// A cap that is not reached shows no badge, so the page does not
  /// alarm a person who is under theirs.
  it('shows no badge while the person is under their cap', async () => {
    mount(body(50, 1))

    expect(await screen.findByText('of a $50.00 monthly cap')).toBeTruthy()
    expect(screen.queryByText('Cap reached')).toBeNull()
  })
})
