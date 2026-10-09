import { useNavigate } from '@tanstack/react-router'
import type { ApiClient } from '../../api/client'
import { Frame, Row, SectionLabel } from '../../primitives'
import {
  useConnections,
  useCredentials,
  useHosts,
  useModelAliases,
  useMySessions,
  useMyUsage,
  usePushSubscriptions,
  useTrustList,
  useWorkspace,
} from '../../queries'
import { visibleSettingsGroups } from '../SettingsPanel'
import { timezoneName } from '../../timezone'
import { NavBar } from './TopBar'
import { useSound } from '../../state/sound'

export function SettingsPhone({ api }: { api: ApiClient }) {
  const navigate = useNavigate()
  const connections = useConnections(api)
  const hosts = useHosts(api)
  const sessions = useMySessions(api)
  const credentials = useCredentials(api)
  const trust = useTrustList(api)
  const push = usePushSubscriptions(api)
  const workspace = useWorkspace(api)
  const usage = useMyUsage(api)
  const aliases = useModelAliases(api)
  const sound = useSound((state) => state.enabled)
  const values: Record<string, string> = {
    models: aliases.data ? `${aliases.data.length} aliases` : '',
    usage: `$${(usage.data?.total.cost_usd ?? 0).toFixed(2)} this month`,
    connections: String(connections.data?.length ?? 0),
    hosts: `${(hosts.data ?? []).filter((row) => row.present).length} of ${hosts.data?.length ?? 0} connected`,
    sessions: String(sessions.data?.length ?? 0),
    vault: `${credentials.data?.length ?? 0} sign-ins`,
    'trusted-contacts': String(trust.data?.items.length ?? 0),
    notifications: push.data?.some((row) => row.current) ? 'On' : 'Off',
    sound: sound ? 'On' : 'Off',
    timezone: workspace.data?.timezone ? timezoneName(workspace.data.timezone) : '',
  }
  return (
    <>
      <NavBar back={{ label: 'You', onBack: () => void navigate({ to: '/you' }) }} />
      <div className="phone-content">
        <h1 className="phone-heading">Settings</h1>
        {visibleSettingsGroups(false).map((group) => (
          <section className="phone-section" key={group.label}>
            <SectionLabel>{group.label}</SectionLabel>
            <Frame>
              {(group.label === 'System' ? [...group.sections].reverse() : group.sections).map(
                (section) => (
                  <Row
                    key={section.value}
                    value={values[section.value]}
                    chevron
                    onClick={() =>
                      void navigate({
                        to: '/settings/$section',
                        params: { section: section.value },
                      })
                    }
                  >
                    {section.label}
                  </Row>
                ),
              )}
            </Frame>
          </section>
        ))}
      </div>
    </>
  )
}
