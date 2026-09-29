// The restart the Administration Interface asks for. The daemon exits
// with the restart code, and what comes next depends on who started it:
// a supervisor (the Client App, or the restart policy of the compose
// deployment) starts it again, and with none a person runs `pagis`
// again. The page reads the System Settings until a new process answers,
// which it knows by its start time, and gives up after a timeout.

import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'

import type { ApiClient } from '../../api/client'
import { systemSettingsKey, useRestartDaemon } from '../../queries'

/** What the page knows about the daemon that runs now. */
export interface RunningDaemon {
  supervised: boolean
  started_at: number
}

export type RestartPhase = 'none' | 'waiting' | 'back' | 'gone'

/** How long a restart may take before the page says it failed. */
export const RESTART_TIMEOUT_MS = 60_000
const POLL_MS = 1_000

export function useDaemonRestart(api: ApiClient, daemon: RunningDaemon) {
  const restart = useRestartDaemon(api)
  const queryClient = useQueryClient()
  const [phase, setPhase] = useState<RestartPhase>('none')
  const startedAt = daemon.started_at

  useEffect(() => {
    if (phase !== 'waiting') return
    const deadline = Date.now() + RESTART_TIMEOUT_MS
    const timer = setInterval(() => {
      if (Date.now() >= deadline) {
        setPhase('gone')
        return
      }
      // A daemon that is down answers no request at all.
      void api
        .GET('/api/v1/settings/system')
        .then(({ data }) => {
          if (data !== undefined && data.started_at !== startedAt) {
            setPhase('back')
            void queryClient.invalidateQueries({ queryKey: systemSettingsKey })
          }
        })
        .catch(() => undefined)
    }, POLL_MS)
    return () => clearInterval(timer)
  }, [api, phase, queryClient, startedAt])

  return {
    ask: () => restart.mutate(undefined, { onSuccess: () => setPhase('waiting') }),
    isPending: restart.isPending,
    phase,
  }
}

/** What the page says in each phase of a restart. */
export function restartMessage(phase: RestartPhase, supervised: boolean): string | null {
  switch (phase) {
    case 'none':
      return null
    case 'waiting':
      return supervised
        ? 'Pagis is starting again.'
        : 'Pagis stopped. Nothing starts it again here: run pagis on this computer.'
    case 'back':
      return 'Pagis is running again.'
    case 'gone':
      return supervised
        ? 'Pagis did not come back. Read the daemon log.'
        : 'Pagis did not come back. Run pagis on this computer to start it.'
  }
}
