// The anonymous analytics of the installation (ADR-0026): one switch,
// on by default. A change takes effect with no restart. A build from
// source, or a daemon with DO_NOT_TRACK set, sends nothing, and the note
// says so.

import type { AnalyticsDto, ApiClient } from '../../api/client'
import { Frame, Row, Switch } from '../../primitives'
import { errorMessage, useSetAnalytics } from '../../queries'

function note(analytics: AnalyticsDto): string {
  if (analytics.blocked === 'build') {
    return 'This build of Pagis sends no analytics. Only a release build sends them.'
  }
  if (analytics.blocked === 'do_not_track') {
    return 'DO_NOT_TRACK is set, so Pagis sends no analytics.'
  }
  if (!analytics.enabled) {
    return 'Pagis sends no analytics.'
  }
  return (
    'Once a day, Pagis sends counts in ranges and the features in use, with a random ' +
    'installation ID. It sends no content, no names and no addresses.'
  )
}

export function Analytics({ api, analytics }: { api: ApiClient; analytics: AnalyticsDto }) {
  const set = useSetAnalytics(api)
  return (
    <Frame>
      <Row>
        <Switch
          checked={analytics.enabled}
          onCheckedChange={(enabled) => !set.isPending && set.mutate(enabled)}
        >
          Send anonymous analytics
        </Switch>
        <span className="system-row-note">
          {set.isError ? (
            <span role="alert" className="system-error">
              {errorMessage(set.error, 'The setting could not be saved.')}
            </span>
          ) : (
            note(analytics)
          )}
        </span>
      </Row>
    </Frame>
  )
}
