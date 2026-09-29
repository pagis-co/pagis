import { useState } from 'react'
import { ArrowUpRight, Download } from 'lucide-react'
import { Button, Select } from '../primitives'
import '../tokens.css'

import { SpriteAvatar } from './SpriteAvatar'
import { AppearanceControls } from './AppearanceControls'
import {
  defaultAppearance,
  spriteCatalog,
  type SpriteAppearance,
} from './catalog'
import type { SpriteClip, SpriteExpression } from './motion'
const manifest = spriteCatalog.pixie
import model from '../../../assets/avatars/pixie/exports/pixie.glb?url'
import blend from '../../../assets/avatars/pixie/pixie.blend?url'
import reference from '../../../assets/avatars/pixie/source/reference.png'
import mint from '../../../assets/avatars/pixie/portraits/mint.png'
import '../../../assets/avatars/pixie/preview/style.css'

const states: { clip: SpriteClip; label: string; caption: string }[] = [
  { clip: 'Idle', label: 'Ready', caption: 'Here for you.' },
  {
    clip: 'Working',
    label: 'Working',
    caption: 'A little focus. A little magic.',
  },
  { clip: 'Waiting', label: 'Waiting', caption: 'Taking a quiet moment.' },
  { clip: 'NeedsInput', label: 'Needs you', caption: 'A little help, please.' },
  { clip: 'Celebrate', label: 'Celebrate', caption: 'We did it!' },
  { clip: 'Error', label: 'Oops', caption: 'Let’s try that again.' },
]

export default function Preview() {
  const [appearance, setAppearance] =
    useState<SpriteAppearance>(defaultAppearance())
  const [clip, setClip] = useState<SpriteClip>('Idle')
  const [expression, setExpression] = useState<
    SpriteExpression | 'Neutral' | undefined
  >()
  const [animate, setAnimate] = useState(true)
  const [take, setTake] = useState(0)
  const preset = manifest.presets[appearance.preset]
  return (
    <main className="studio">
      <header className="studio-header">
        <a href="/pixie.html" className="wordmark">
          pagis<span> / little sprites</span>
        </a>
        <span className="edition">CHARACTER 01</span>
      </header>
      <div className="studio-intro">
        <p className="eyebrow">PIXIE · CURIOUS HEART</p>
        <h1>
          A little life.
          <br />
          <em>Yours to shape.</em>
        </h1>
        <p>Leaf hair, bright eyes, and a pocket full of possibilities.</p>
      </div>
      <div className="studio-layout">
        <section className="stage" aria-label="Interactive pixie preview">
          <div className="stage-halo" />
          <span className="stage-label">
            {preset.label}
            <span>
              {appearance.preset === 'mint'
                ? 'The original curious heart'
                : appearance.preset === 'lavender'
                  ? 'A thoughtful little reader'
                  : 'A warm and cozy companion'}
            </span>
          </span>
          <SpriteAvatar
            key={take}
            name="Pixie"
            appearance={appearance}
            clip={clip}
            expression={expression}
            animate={animate}
            interactive
            className="studio-character"
          />
          <div className="stage-caption">
            <span className="live-dot" />
            {states.find((state) => state.clip === clip)?.caption}
          </div>
          <p className="stage-hint">Drag to turn · Scroll to look closer</p>
        </section>
        <aside className="controls">
          <section>
            <div className="section-title">
              <span>01</span>
              <h2>Make it yours</h2>
            </div>
            <AppearanceControls value={appearance} onChange={setAppearance} />
          </section>
          <section>
            <div className="section-title">
              <span>02</span>
              <h2>A little personality</h2>
            </div>
            <div className="state-list">
              {states.map((state) => (
                <Button
                  key={state.clip}
                  size="sm"
                  variant={clip === state.clip ? 'primary' : 'outline'}
                  aria-pressed={clip === state.clip}
                  onClick={() => {
                    setClip(state.clip)
                    setTake((value) => value + 1)
                  }}
                >
                  {state.label}
                </Button>
              ))}
            </div>
            <div className="expression">
              <span>Expression</span>
              <Select
                label="Expression"
                value={expression ?? 'Auto'}
                onValueChange={(value) =>
                  setExpression(
                    value === 'Auto'
                      ? undefined
                      : (value as SpriteExpression | 'Neutral'),
                  )
                }
                items={[
                  { value: 'Auto', label: 'Follow the animation' },
                  ...['Neutral', 'Smile', 'Surprise', 'Concern', 'Blink'].map(
                    (value) => ({ value, label: value }),
                  ),
                ]}
              />
            </div>
            <label className="motion">
              <input
                type="checkbox"
                checked={animate}
                onChange={(event) => setAnimate(event.target.checked)}
              />
              Play animation
            </label>
          </section>
          <section className="take-home">
            <div className="section-title">
              <span>03</span>
              <h2>Ready to go</h2>
            </div>
            <p>
              One character. Six animations. Three colorways.
              <br />
              Keep shaping it in Blender, or bring it to the web.
            </p>
            <div className="downloads">
              <a href={model} download="pixie.glb">
                Download GLB <Download size={14} aria-hidden />
              </a>
              <a href={blend} download="pixie.blend">
                Blender source <ArrowUpRight size={14} aria-hidden />
              </a>
            </div>
            <p className="download-note">
              Downloads contain the mint base and all accessories. Appearance
              changes above are a preview.
            </p>
          </section>
        </aside>
      </div>
      <section className="reference-check" aria-label="Compare the design">
        <h2>Compare the design</h2>
        <div className="reference-comparison">
          <figure>
            <div className="reference-crop">
              <img src={reference} alt="The original Pixie illustration" />
            </div>
            <figcaption>Your illustration</figcaption>
          </figure>
          <figure>
            <img
              className="reference-render"
              src={mint}
              alt="The revised Mint Pixie in Blender"
            />
            <figcaption>Current Blender model</figcaption>
          </figure>
        </div>
      </section>
      <footer>
        <span>Small companion. Room to grow.</span>
        <span>Made for Pagis</span>
      </footer>
    </main>
  )
}
