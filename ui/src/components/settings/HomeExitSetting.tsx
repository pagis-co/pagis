// The Home Exit of a server (ADR-0029): one switch, on by default, with
// which the Administrator turns the Home Exit off for every Person of the
// server. It never turns a Home Exit on for a Person: each Person chooses
// their own in Settings. A change takes effect at once, with no restart:
// each awake Computer whose mode changes switches.

import type { ApiClient, HomeExitSettingDto } from '../../api/client'
import { Frame, Row, Switch } from '../../primitives'
import { errorMessage, useSetHomeExitSetting } from '../../queries'

function note(homeExit: HomeExitSettingDto): string {
  if (!homeExit.enabled) {
    return (
      "Every sprite's computer reaches the internet from this server. Each Person's choice " +
      'stays, and it is in effect again when you turn the Home Exit on.'
    )
  }
  return (
    "A Person can send the connections of their sprites' computers through a computer of " +
    'their own, so that sites see their own address. Each Person turns it on for their own ' +
    'sprites in Settings.'
  )
}

export function HomeExitSetting({
  api,
  homeExit,
}: {
  api: ApiClient
  homeExit: HomeExitSettingDto
}) {
  const set = useSetHomeExitSetting(api)
  const notSwitched = set.data?.not_switched ?? 0
  return (
    <Frame>
      <Row>
        <Switch
          checked={homeExit.enabled}
          onCheckedChange={(enabled) => !set.isPending && set.mutate(enabled)}
        >
          Let People use a Home Exit
        </Switch>
        <span className="system-row-note">
          {set.isError ? (
            <span role="alert" className="system-error">
              {errorMessage(set.error, 'The setting could not be saved.')}
            </span>
          ) : (
            <>
              {note(homeExit)}
              {notSwitched > 0 &&
                ` ${notSwitched} ${notSwitched === 1 ? 'computer' : 'computers'} did not switch; each one takes the setting when it wakes again.`}
            </>
          )}
        </span>
      </Row>
    </Frame>
  )
}
