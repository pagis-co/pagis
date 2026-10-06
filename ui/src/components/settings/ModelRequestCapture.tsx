// Model Request Capture (ADR-0031): one switch, off by default, and the
// number of days Pagis keeps each copy. A change takes effect with no
// restart, and turning the switch off deletes every copy.

import { useState } from 'react'

import type { ApiClient, ModelRequestCaptureSettingDto } from '../../api/client'
import { Button, Frame, Input, Row, Switch } from '../../primitives'
import { errorMessage, useSetModelRequestCapture } from '../../queries'

const MAX_DAYS = 30

function note(capture: ModelRequestCaptureSettingDto): string {
  if (!capture.enabled) return 'Pagis keeps no copy of a model request.'
  return (
    `Pagis keeps a copy of each model request for ${capture.retention_days} days: the ` +
    'messages, the memory and the tool results that the request sent, and the answer of the ' +
    'provider. Only an Administrator reads it, on the page of a Run. Turn this off to delete ' +
    'every copy.'
  )
}

export function ModelRequestCapture({
  api,
  capture,
}: {
  api: ApiClient
  capture: ModelRequestCaptureSettingDto
}) {
  const set = useSetModelRequestCapture(api)
  const [days, setDays] = useState(String(capture.retention_days))
  const parsed = Number(days)
  const valid = Number.isInteger(parsed) && parsed >= 1 && parsed <= MAX_DAYS
  const changed = parsed !== capture.retention_days

  return (
    <Frame>
      <Row>
        <Switch
          checked={capture.enabled}
          onCheckedChange={(enabled) =>
            !set.isPending &&
            set.mutate({ enabled, retention_days: capture.retention_days })
          }
        >
          Keep a copy of each model request
        </Switch>
        <span className="system-row-note">
          {set.isError ? (
            <span role="alert" className="system-error">
              {errorMessage(set.error, 'The setting could not be saved.')}
            </span>
          ) : (
            note(capture)
          )}
        </span>
      </Row>
      <Row>
        <Input
          type="number"
          min={1}
          max={MAX_DAYS}
          aria-label="Days to keep a copy"
          value={days}
          onChange={(event) => setDays(event.target.value)}
        />
        <span className="system-row-note">days, from 1 to {MAX_DAYS}</span>
        <Button
          size="sm"
          aria-label="Save the retention"
          disabled={!valid || !changed || set.isPending}
          onClick={() => set.mutate({ enabled: capture.enabled, retention_days: parsed })}
        >
          Save
        </Button>
      </Row>
    </Frame>
  )
}
