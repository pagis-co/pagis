// The mutations whose route answers 204 No Content. openapi-fetch
// answers such a request with no `data`, so a mutation that waits for
// `data` fails after the daemon did the work, and the page it should
// refresh keeps the old state.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, renderHook, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from './api/client'
import {
  callKey,
  createQueryClient,
  needsYouKey,
  trustListKey,
  useClearKeypadFailures,
  useDeleteKeypadCode,
  useDeleteTrustEntry,
  useHangUpCall,
  useTimeline,
} from './queries'

/** What openapi-fetch answers for a 204: no `data` and no `error`. */
function noContent() {
  return { data: undefined, error: undefined, response: new Response(null, { status: 204 }) }
}

/** What openapi-fetch answers for a refusal with a JSON body. */
function refused(status: number, message: string) {
  return {
    data: undefined,
    error: { error: { message } },
    response: new Response(null, { status }),
  }
}

function stubApi(answer: () => unknown) {
  return {
    POST: vi.fn(async () => answer()),
    DELETE: vi.fn(async () => answer()),
  }
}

/** A hook under a fresh query client, and the spy on its invalidations. */
function mount<T>(hook: () => T) {
  const queryClient = new QueryClient({ defaultOptions: { mutations: { retry: false } } })
  const invalidated = vi.spyOn(queryClient, 'invalidateQueries')
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  )
  const { result } = renderHook(hook, { wrapper })
  return { result, invalidated }
}

describe('a mutation of a 204 route', () => {
  it('deletes a Trust List entry and refreshes the list', async () => {
    const api = stubApi(noContent)
    const { result, invalidated } = mount(() => useDeleteTrustEntry(api as unknown as ApiClient))

    await act(() => result.current.mutateAsync('row-1'))

    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/settings/trust-list/{trust_entry_id}', {
      params: { path: { trust_entry_id: 'row-1' } },
    })
    expect(invalidated).toHaveBeenCalledWith({ queryKey: trustListKey })
  })

  it('deletes the Keypad Code and refreshes the list', async () => {
    const api = stubApi(noContent)
    const { result, invalidated } = mount(() => useDeleteKeypadCode(api as unknown as ApiClient))

    await act(() => result.current.mutateAsync())

    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/settings/keypad-code')
    expect(invalidated).toHaveBeenCalledWith({ queryKey: trustListKey })
  })

  it('clears the failed attempts of the Keypad Code and refreshes the list and the queue', async () => {
    const api = stubApi(noContent)
    const { result, invalidated } = mount(() =>
      useClearKeypadFailures(api as unknown as ApiClient),
    )

    await act(() => result.current.mutateAsync())

    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/settings/keypad-code/failures')
    expect(invalidated).toHaveBeenCalledWith({ queryKey: trustListKey })
    expect(invalidated).toHaveBeenCalledWith({ queryKey: needsYouKey })
  })

  it('hangs up a live call and refreshes the call', async () => {
    const api = stubApi(noContent)
    const { result, invalidated } = mount(() =>
      useHangUpCall(api as unknown as ApiClient, 'call-1'),
    )

    await act(() => result.current.mutateAsync())

    expect(api.POST).toHaveBeenCalledWith('/api/v1/calls/{call_id}/hangup', {
      params: { path: { call_id: 'call-1' } },
    })
    expect(invalidated).toHaveBeenCalledWith({ queryKey: callKey('call-1') })
  })

  it('still fails on a refusal, and refreshes nothing', async () => {
    const api = stubApi(() => refused(404, 'that listed contact was not found'))
    const { result, invalidated } = mount(() => useDeleteTrustEntry(api as unknown as ApiClient))

    await expect(act(() => result.current.mutateAsync('row-1'))).rejects.toEqual({
      error: { message: 'that listed contact was not found' },
    })
    expect(invalidated).not.toHaveBeenCalled()
  })
})

// A conversation address that names no conversation of the person
// answers 404. The 404 is an answer, not a failure, so the page's query
// client does not ask again and the view says at once that the
// conversation does not exist. Other failures still retry.
describe('the query client of a page', () => {
  const page = { data: { items: [] }, error: undefined, response: new Response(null, { status: 200 }) }
  const notFound = {
    data: undefined,
    error: { error: { code: 'not_found', message: 'channel not found' } },
    response: new Response(null, { status: 404 }),
  }

  /** The Timeline under the page's query client, with no delay
   *  between the attempts. */
  function mountTimeline(GET: ReturnType<typeof vi.fn>) {
    const queryClient = createQueryClient()
    queryClient.setDefaultOptions({
      queries: { ...queryClient.getDefaultOptions().queries, retryDelay: 0 },
    })
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    )
    const api = { GET } as unknown as ApiClient
    return renderHook(() => useTimeline(api, 'channel-1'), { wrapper }).result
  }

  it('does not retry a conversation that is not found', async () => {
    const GET = vi.fn(async () => notFound)
    const result = mountTimeline(GET)

    await waitFor(() => expect(result.current.isError).toBe(true))
    expect(GET).toHaveBeenCalledTimes(1)
  })

  it('retries another failure', async () => {
    const GET = vi
      .fn()
      .mockResolvedValueOnce(refused(500, 'internal error'))
      .mockResolvedValueOnce(page)
    const result = mountTimeline(GET)

    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    expect(GET).toHaveBeenCalledTimes(2)
    expect(result.current.data).toEqual([])
  })

  it('stops after three retries', async () => {
    const GET = vi.fn(async () => refused(500, 'internal error'))
    const result = mountTimeline(GET)

    await waitFor(() => expect(result.current.isError).toBe(true))
    expect(GET).toHaveBeenCalledTimes(4)
  })
})
