import { useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { BookOpen, Clock3, Lock, MessageSquare, Moon, Phone } from 'lucide-react'
import type { AgentDto, ApiClient } from '../../api/client'
import { Avatar, Badge, Button, Frame, Row, Sheet } from '../../primitives'
import {
  useChannels,
  useComputer,
  useGrants,
  usePhoneNumbers,
  useRecentChanges,
  useRuns,
  useUpdateAgent,
  useUpdateAppearance,
  useWorkspace,
} from '../../queries'
import { agentPresence, liveCallOf, usePresence } from '../../state/presence'
import { LiveScreen } from '../LiveScreen'
import { AgentForm } from './AgentForm'
import { AppearanceControls } from '../../avatars/AppearanceControls'
import { AgentPhoneNumber } from '../AgentPhoneNumber'
import { AgentMailbox } from '../AgentMailbox'
import { directMessageChannel } from '../AskAnAgent'
import { NavBar } from '../phone/TopBar'
import { activityTone, activityWord, computerStateWord } from '../stateWords'
import { changeTimeLabel } from '../memory/pages'

export function AgentProfilePhone({ api, agent }: { api: ApiClient; agent: AgentDto }) {
  const navigate = useNavigate()
  const workspace = useWorkspace(api)
  const channels = useChannels(api)
  const computer = useComputer(api, agent.id)
  const grants = useGrants(api)
  const numbers = usePhoneNumbers(api)
  const changes = useRecentChanges(api, `agent:${agent.id}`, 1)
  const runs = useRuns(api, agent.id, '', '')
  const update = useUpdateAgent(api)
  const updateAppearance = useUpdateAppearance(api)
  const presence = usePresence((state) => agentPresence(state, agent.id))
  const call = usePresence((state) => liveCallOf(state, agent.id))
  const caption = usePresence(
    (state) => Object.values(state.runs).find((run) => run.agentId === agent.id)?.caption ?? null,
  )
  const [editing, setEditing] = useState(false)
  const [appearance, setAppearance] = useState(agent.avatar)
  const phone = numbers.data?.items.find((row) => row.agent_id === agent.id)?.e164
  const owned = (grants.data ?? []).filter((row) => row.agent_id === agent.id)
  const accounts = owned.filter((row) => row.resource_kind === 'connection').length
  const hosts = owned.filter((row) => row.resource_kind === 'host').length
  const change = changes.data?.items[0]
  const run = runs.data?.[0]
  const state = computer.data?.state ?? 'off'
  const deskLine =
    state === 'off'
      ? `Asleep. It wakes when ${agent.name} starts work.`
      : (caption ?? computerStateWord(state, computer.data?.percent))
  return (
    <>
      <NavBar
        back={{ label: 'Sprites', onBack: () => void navigate({ to: '/sprites' }) }}
        actions={
          <Button variant="link" onClick={() => setEditing(true)}>
            Edit
          </Button>
        }
      />
      <div className="phone-content" data-testid="agent-profile">
        <header className="phone-portrait-head">
          <Avatar
            id={agent.id}
            name={agent.name}
            appearance={agent.avatar}
            size="portrait"
            presence={presence}
          />
          <h1 className="phone-heading">{agent.name}</h1>
          <p className="phone-hint">{agent.job}</p>
          <div className="phone-badges">
            <Badge tone={activityTone(presence)}>{activityWord(presence)}</Badge>
            {workspace.data?.chief_of_staff_agent_id === agent.id && <Badge>Main sprite</Badge>}
            {phone && <Badge>{phone}</Badge>}
            {agent.email_address && <Badge>{agent.email_address}</Badge>}
          </div>
        </header>
        <div className="phone-actions">
          <Frame>
            <Row
              onClick={() => {
                const channelId = directMessageChannel(channels.data ?? [], agent.id)
                if (channelId) void navigate({ to: '/c/$channelId', params: { channelId } })
              }}
            >
              <span className="phone-profile-action">
                <MessageSquare size={20} aria-hidden />
                Message
              </span>
            </Row>
          </Frame>
          {phone && (
            <Frame>
              <Row
                disabled={!call}
                onClick={() => {
                  if (call) void navigate({ to: '/calls/$callId', params: { callId: call } })
                }}
              >
                <span className="phone-profile-action">
                  <Phone size={20} aria-hidden />
                  Call
                </span>
              </Row>
            </Frame>
          )}
        </div>
        <Frame>
          <Row>
            <div className="phone-row-copy">
              <strong className="phone-hint">What to ask it for</strong>
              <p className="phone-profile-description">
                {agent.description || 'No description written yet.'}
              </p>
            </div>
          </Row>
        </Frame>
        <Frame>
          <Row
            chevron
            onClick={() =>
              void navigate({ to: '/sprites/$agentId/desk', params: { agentId: agent.id } })
            }
          >
            <span className="phone-desk-preview">
              {state === 'awake' ? (
                <LiveScreen
                  api={api}
                  agentId={agent.id}
                  agentName={agent.name}
                  mode="compact"
                  fallback={<Moon size={20} aria-hidden />}
                />
              ) : (
                <Moon size={20} aria-hidden />
              )}
            </span>
            <span className="phone-row-copy">
              <strong>{agent.name}'s desk</strong>
              <span className="phone-hint">{deskLine}</span>
            </span>
          </Row>
        </Frame>
        <Frame>
          <Row
            chevron
            onClick={() =>
              void navigate({ to: '/sprites/$agentId/access', params: { agentId: agent.id } })
            }
          >
            <Lock size={20} aria-hidden />
            <span className="phone-row-copy">
              <span>Access</span>
              <span className="phone-hint">
                {accounts} {accounts === 1 ? 'account' : 'accounts'} and {hosts}{' '}
                {hosts === 1 ? 'computer' : 'computers'}
              </span>
            </span>
          </Row>
          <Row
            chevron
            onClick={() =>
              void navigate({ to: '/sprites/$agentId/memory', params: { agentId: agent.id } })
            }
          >
            <BookOpen size={20} aria-hidden />
            <span className="phone-row-copy">
              <span>Memory</span>
              <span className="phone-hint">{change ? change.message : 'No change yet.'}</span>
            </span>
          </Row>
          <Row
            chevron
            onClick={() =>
              void navigate({ to: '/sprites/$agentId/work', params: { agentId: agent.id } })
            }
          >
            <Clock3 size={20} aria-hidden />
            <span className="phone-row-copy">
              <span>Work</span>
              <span className="phone-hint">
                {run ? `${run.title} · ${changeTimeLabel(run.created_at)}` : 'No run yet.'}
              </span>
            </span>
          </Row>
        </Frame>
      </div>
      <Sheet open={editing} onOpenChange={setEditing} title={`Edit ${agent.name}`}>
        <div className="phone-form">
          <AppearanceControls value={appearance} onChange={setAppearance} compact />
          <AgentForm
            api={api}
            initial={{ ...agent, voice: agent.voice ?? null }}
            pending={update.isPending || updateAppearance.isPending}
            failure={update.error ?? updateAppearance.error}
            submitLabel="Save"
            onSubmit={async (fields) => {
              try {
                await update.mutateAsync({ agentId: agent.id, ...fields })
                await updateAppearance.mutateAsync({ agentId: agent.id, avatar: appearance })
                setEditing(false)
              } catch {
                /* The form shows the failed mutation. */
              }
            }}
          />
          <AgentPhoneNumber api={api} agent={agent} />
          <AgentMailbox api={api} agent={agent} />
        </div>
      </Sheet>
    </>
  )
}
