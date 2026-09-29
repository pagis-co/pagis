// The Needs-You Queue, live. The runs that wait
// come from the presence store, which the WS firehose folds frame by
// frame; the approvals, the failures, the missed calls and the keypad
// delay come from queries the same firehose invalidates. Home draws the
// queue and the sidebar counts it, so both read this one hook.

import { useMemo } from 'react'

import type { ApiClient } from '../../api/client'
import { useCalls, usePendingRequests, useRuns, useTrustList } from '../../queries'
import { usePresence } from '../../state/presence'
import { buildQueue, type QueueItem } from './queue'

export interface Queue {
  items: QueueItem[]
  /** One of the four lists has not landed yet. */
  isPending: boolean
  /** One of the four lists failed. */
  isError: boolean
  refetch: () => void
}

export function useQueue(api: ApiClient): Queue {
  const requests = usePendingRequests(api)
  const failed = useRuns(api, '', '', 'failed')
  const endedCalls = useCalls(api, '', 'inbound', 'ended')
  // The Keypad Code card holds the failed-attempt count (ADR-0021).
  const trustList = useTrustList(api)
  const liveRuns = usePresence((state) => state.runs)

  const items = useMemo(
    () =>
      buildQueue({
        requests: requests.data ?? [],
        liveRuns: Object.values(liveRuns),
        failedRuns: failed.data ?? [],
        calls: endedCalls.data ?? [],
        keypad: trustList.data?.keypad_code ?? null,
      }),
    [requests.data, liveRuns, failed.data, endedCalls.data, trustList.data],
  )

  return {
    items,
    isPending:
      requests.isPending || failed.isPending || endedCalls.isPending || trustList.isPending,
    isError: requests.isError || failed.isError || endedCalls.isError || trustList.isError,
    refetch: () => {
      void requests.refetch()
      void failed.refetch()
      void endedCalls.refetch()
      void trustList.refetch()
    },
  }
}
