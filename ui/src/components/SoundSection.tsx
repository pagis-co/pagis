// The Sound section: one switch, then the four cues with a way
// to hear each. The setting stays in this browser, so the section asks
// the store and never the daemon.

import { Button, Frame, IconButton, Row, SectionLabel, Switch } from '../primitives'
import { CirclePlay } from 'lucide-react'
import { useIsMobile } from '../state/useIsMobile'
import { CuePlayer, type CueName } from '../sound/cues'
import { useSound } from '../state/sound'

import './SoundSection.css'

/** Each cue: its name and the moment it marks, in the reader's words. */
const CUES: ReadonlyArray<{ cue: CueName; name: string; meaning: string }> = [
  { cue: 'approval', name: 'Approval needed', meaning: 'something waits for you' },
  { cue: 'ringing', name: 'Call ringing', meaning: 'an inbound call before a sprite answers' },
  { cue: 'connected', name: 'Call connected', meaning: 'the call is live' },
  { cue: 'failed', name: 'Run failed', meaning: 'a run ended failed' },
]

/** The preview player. It is not the one the cues play through, so a
 *  preview sounds while the setting is on and the reader listens. */
const preview = new CuePlayer()

export function SoundSection() {
  const phone = useIsMobile()
  const enabled = useSound((state) => state.enabled)
  const setEnabled = useSound((state) => state.setEnabled)

  if (phone) return <section className="phone-section" data-testid="sound-settings"><div><h1 className="phone-heading">Sound</h1><p className="phone-lead">Four cues, all optional.</p></div><Frame><Switch row checked={enabled} onCheckedChange={setEnabled}><span className="phone-row-copy"><span>Play sound cues</span><span className="phone-hint">Off until you turn it on.</span></span></Switch></Frame><SectionLabel>Cues</SectionLabel><Frame>{CUES.map(({ cue, name }) => <Row key={cue}><span className="phone-row-copy">{name}</span><IconButton variant="link" className="phone-accent" icon={CirclePlay} label={`Hear ${name}`} onClick={() => preview.play(cue)} /></Row>)}</Frame></section>

  return (
    <section className="sound-section" data-testid="sound-settings">
      <div className="sound-title">
        <h1>Sound</h1>
        <span>Four cues, all optional.</span>
      </div>
      <Frame>
        <Row>
          <Switch checked={enabled} onCheckedChange={setEnabled}>
            Play sound cues
          </Switch>
          <span className="sound-note">Off until you turn it on.</span>
        </Row>
      </Frame>
      <Frame hint="The cues follow the theme: quiet on a warm ground. Reduced motion does not silence them; the switch does.">
        {CUES.map(({ cue, name, meaning }) => (
          <Row key={cue}>
            <span className="sound-cue-name">{name}</span>
            <span className="sound-cue-meaning">{meaning}</span>
            <Button
              size="sm"
              className="sound-hear"
              disabled={!enabled}
              aria-label={`Hear the cue for: ${name}`}
              onClick={() => preview.play(cue)}
            >
              Hear
            </Button>
          </Row>
        ))}
      </Frame>
    </section>
  )
}
