// The Person's Stop of a Coding Session (ADR-0033). It is the brake of a
// session that runs, so it asks for no confirmation, as Hang up does.
// The session block and the head of the session page show it while the
// session is not settled.

import type { ApiClient } from '../../api/client'
import { Button } from '../../primitives'
import { errorMessage, useStopCodingSession } from '../../queries'

import './coding.css'

export function StopCodingSession({ api, sessionId }: { api: ApiClient; sessionId: string }) {
  const stop = useStopCodingSession(api, sessionId)
  return (
    <span className="coding-stop">
      <Button
        variant="danger"
        size="sm"
        disabled={stop.isPending}
        onClick={() => stop.mutate()}
      >
        Stop
      </Button>
      {stop.isError && (
        <span className="coding-stop-error" role="alert">
          {errorMessage(stop.error, 'The coding session did not stop.')}
        </span>
      )}
    </span>
  )
}
