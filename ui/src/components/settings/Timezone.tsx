import { useState } from 'react'

import type { ApiClient } from '../../api/client'
import { Button, Frame, Row, Select } from '../../primitives'
import { errorMessage, useSetTimezone, useWorkspace } from '../../queries'
import { deviceTimezone, knownTimezones } from '../../timezone'

import './Timezone.css'

/** The Timezone section: the Person's own clock. New Schedules copy it,
 *  and the Daily report comes at 7:00 on it. The first sign-in takes it
 *  from the device; here the Person changes it. */
export function TimezoneSection({ api }: { api: ApiClient }) {
  const workspace = useWorkspace(api)
  if (!workspace.data) {
    return (
      <section className="settings-timezone">
        <div className="settings-timezone-title">
          <h1>Timezone</h1>
        </div>
        <p className="settings-timezone-note">
          {workspace.isError ? 'The timezone could not be read.' : 'Reading…'}
        </p>
      </section>
    )
  }
  return <TimezoneForm api={api} saved={workspace.data.timezone} />
}

function TimezoneForm({ api, saved }: { api: ApiClient; saved: string }) {
  const set = useSetTimezone(api)
  const [timezone, setTimezone] = useState(saved)
  const device = deviceTimezone()

  return (
    <section className="settings-timezone">
      <div className="settings-timezone-title">
        <h1>Timezone</h1>
        <span>Your Schedules and your Daily report run on this clock.</span>
      </div>
      <Frame hint="A Schedule you made before keeps its own timezone. The Daily report moves with this one.">
        <Row>
          <Select
            label="Timezone"
            value={timezone}
            onValueChange={setTimezone}
            items={knownTimezones(timezone).map((zone) => ({ value: zone, label: zone }))}
          />
          {device && device !== timezone && (
            <Button size="sm" variant="ghost" onClick={() => setTimezone(device)}>
              Use this device's timezone ({device})
            </Button>
          )}
          <Button
            size="sm"
            variant="primary"
            className="settings-timezone-save"
            disabled={timezone === saved || set.isPending}
            onClick={() => set.mutate(timezone)}
          >
            Save
          </Button>
        </Row>
        {set.isError && (
          <Row>
            <span className="settings-timezone-error" role="alert">
              {errorMessage(set.error, 'That timezone could not be saved.')}
            </span>
          </Row>
        )}
      </Frame>
    </section>
  )
}
