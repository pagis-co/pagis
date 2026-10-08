import { useState } from 'react'
import { ChevronLeft } from 'lucide-react'
import type { ApiClient } from '../../api/client'
import {
  Avatar,
  Button,
  Frame,
  Input,
  Row,
  SectionLabel,
  Sheet,
  Switch,
  Textarea,
} from '../../primitives'
import { defaultAppearance, spriteCatalog } from '../../avatars/catalog'
import { errorMessage, useConnections } from '../../queries'
import type { NewSpriteDraft } from '../sprites/NewSprite'
import { VoicePicker } from '../sprites/VoicePicker'
import { MailboxFields, NoMailboxProvider, type MailboxOffer } from '../AgentMailbox'
import { CapabilityPicker } from '../ConnectionCapabilities'

export function NewSpriteSheet({
  api,
  open,
  onClose,
  draft,
  onChange,
  busy,
  failure,
  offers,
  onMailboxChange,
  onCreate,
}: {
  api: ApiClient
  open: boolean
  onClose: () => void
  draft: NewSpriteDraft
  onChange: (next: Partial<NewSpriteDraft>) => void
  busy: boolean
  failure: unknown
  offers: MailboxOffer[]
  onMailboxChange: (mailbox: NewSpriteDraft['mailbox']) => void
  onCreate: () => void
}) {
  const [step, setStep] = useState<'main' | 'voice' | 'access'>('main')
  const connections = useConnections(api)
  return (
    <Sheet
      open={open}
      onOpenChange={(next) => {
        if (!next) {
          setStep('main')
          onClose()
        }
      }}
      title={
        step === 'main'
          ? 'New sprite'
          : step === 'voice'
            ? 'Personality and voice'
            : 'What it may reach'
      }
      action={{
        label: step === 'main' ? 'Create' : 'Done',
        onSelect: step === 'main' ? onCreate : () => setStep('main'),
        disabled: busy || !draft.name.trim() || !draft.job.trim(),
      }}
    >
      <div className="phone-form">
        {step !== 'main' && (
          <Button variant="link" onClick={() => setStep('main')}>
            <ChevronLeft size={16} aria-hidden />
            New sprite
          </Button>
        )}
        {step === 'main' ? (
          <>
            <section className="phone-section">
              <SectionLabel>Face</SectionLabel>
              <div className="phone-sprite-faces">
                {Object.entries(spriteCatalog).map(([id, sprite]) => (
                  <label className="phone-sprite-face" key={id}>
                    <input
                      type="radio"
                      name="sprite-face"
                      checked={draft.avatar.sprite === id}
                      onChange={() => onChange({ avatar: defaultAppearance(id) })}
                    />
                    <Avatar
                      id={`new-${id}`}
                      name={sprite.label}
                      appearance={defaultAppearance(id)}
                      size="face"
                    />
                    <span>{sprite.label}</span>
                  </label>
                ))}
              </div>
            </section>
            <div className="phone-form-field">
              <label htmlFor="new-sprite-name">Name</label>
              <Input
                id="new-sprite-name"
                value={draft.name}
                onChange={(event) => onChange({ name: event.target.value })}
              />
            </div>
            <div className="phone-form-field">
              <label htmlFor="new-sprite-job">Job</label>
              <Input
                id="new-sprite-job"
                value={draft.job}
                onChange={(event) => onChange({ job: event.target.value })}
              />
            </div>
            <div className="phone-form-field">
              <label htmlFor="new-sprite-description">What to ask it for</label>
              <Textarea
                id="new-sprite-description"
                rows={3}
                value={draft.description}
                onChange={(event) => onChange({ description: event.target.value })}
              />
              <p className="phone-hint">
                Other sprites read this to know what to send to {draft.name || 'this sprite'}. Name
                one kind of work: “personal email”, not “email”.
              </p>
            </div>
            <Frame>
              <Row chevron value="Optional" onClick={() => setStep('voice')}>
                Personality and voice
              </Row>
              <Row
                chevron
                value={
                  Object.values(draft.capabilities).some((items) => items.length)
                    ? 'Access selected'
                    : 'Nothing yet'
                }
                onClick={() => setStep('access')}
              >
                What it may reach
              </Row>
            </Frame>
          </>
        ) : step === 'voice' ? (
          <>
            <div className="phone-form-field">
              <label htmlFor="new-sprite-personality">Personality</label>
              <Textarea
                id="new-sprite-personality"
                rows={4}
                value={draft.personality}
                onChange={(event) => onChange({ personality: event.target.value })}
              />
            </div>
            <VoicePicker api={api} value={draft.voice} onChange={(voice) => onChange({ voice })} />
          </>
        ) : (
          <>
            {(connections.data ?? []).map((connection) => (
              <section className="phone-section" key={connection.id}>
                <strong>{connection.display_name}</strong>
                <p className="phone-hint">{connection.account ?? connection.alias}</p>
                <CapabilityPicker
                  selected={draft.capabilities[connection.id] ?? []}
                  onChange={(capabilities) =>
                    onChange({
                      capabilities: { ...draft.capabilities, [connection.id]: capabilities },
                    })
                  }
                />
              </section>
            ))}
            {connections.data?.length === 0 && (
              <p className="phone-hint">No connection yet. Connect an account in Settings.</p>
            )}
            <section className="phone-section">
              <SectionLabel>Mailbox</SectionLabel>
              {offers.length ? (
                <>
                  <Frame>
                    <Switch
                      row
                      checked={draft.mailboxWanted}
                      onCheckedChange={(mailboxWanted) => onChange({ mailboxWanted })}
                    >
                      Give {draft.name.trim() || 'this sprite'} its own mailbox
                    </Switch>
                  </Frame>
                  {draft.mailboxWanted && (
                    <MailboxFields
                      api={api}
                      offers={offers}
                      draft={draft.mailbox}
                      onChange={onMailboxChange}
                    />
                  )}
                </>
              ) : (
                <NoMailboxProvider api={api} />
              )}
            </section>
          </>
        )}
        {failure != null && (
          <p role="alert">{errorMessage(failure, 'That sprite could not be made.')}</p>
        )}
      </div>
    </Sheet>
  )
}
