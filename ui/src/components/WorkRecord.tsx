// The work record under one reply. The Run states what it did
// in one line — how long it worked, how many steps it took, and which
// tool families it touched. The line opens to the steps, and a step
// that took a screen carries a tile that scrolls the Desk panel to
// that screenshot.
//
// The steps come from `GET /runs/{run_id}/steps`. A finished
// Run never changes, so the answer is kept for the session; a live Run
// is dropped by the `run.*` frames the shell folds.

import { useState } from 'react'
import { ChevronDown, ChevronRight } from 'lucide-react'

import type { ApiClient, RunStepDto, RunStepsDto } from '../api/client'
import { Button } from '../primitives'
import { useRunSteps } from '../queries'
import { formatDuration, type WorkSummary } from '../timeline'

import './WorkRecord.css'

/** The word one tool family goes by in the line. */
const FAMILY: Record<RunStepDto['kind'], string> = {
  memory: 'memory',
  mail: 'mail',
  calendar: 'calendar',
  desk: 'the desk',
  plugin: 'plugins',
}

/** The families the Run touched, in the order it touched them. */
export function families(steps: readonly RunStepDto[]): string[] {
  const seen: string[] = []
  for (const step of steps) {
    const word = FAMILY[step.kind]
    if (!seen.includes(word)) seen.push(word)
  }
  return seen
}

/** `Worked 1 min 5 s · 6 steps · mail, calendar`. The parts that are
 *  not known yet are left out, so the line is true while it loads. */
export function recordLabel(workedMs: number, steps: RunStepsDto | undefined): string {
  const parts = [`Worked ${formatDuration(steps?.worked_ms ?? workedMs)}`]
  if (steps !== undefined) {
    const count = steps.steps.length
    parts.push(count === 1 ? '1 step' : `${count} steps`)
    const touched = families(steps.steps)
    if (touched.length > 0) parts.push(touched.join(', '))
  }
  return parts.join(' · ')
}

export function WorkRecord({
  api,
  work,
  onShowScreenshot,
}: {
  api: ApiClient
  work: WorkSummary
  /** Scrolls the Desk panel to the screenshot of one step. */
  onShowScreenshot?: (screenshotId: string) => void
}) {
  const [open, setOpen] = useState(false)
  const steps = useRunSteps(api, work.runId)
  return (
    <div className="work-record" data-testid="work-record">
      <Button
        size="sm"
        variant="outline"
        className="work-record-line"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {open ? <ChevronDown size={14} aria-hidden /> : <ChevronRight size={14} aria-hidden />}
        {recordLabel(work.endedAt - work.startedAt, steps.data)}
      </Button>
      {open && (
        <ol className="work-record-steps">
          {steps.data?.steps.map((step) => (
            <li className="work-record-step" key={step.index}>
              <span className="work-record-index">{step.index}</span>
              <span className="work-record-label">{step.label}</span>
              {step.screenshot_id != null && onShowScreenshot !== undefined && (
                <Button
                  size="sm"
                  variant="link"
                  className="work-record-tile"
                  onClick={() => onShowScreenshot(step.screenshot_id as string)}
                >
                  <span className="work-record-thumb" aria-hidden />
                  see
                </Button>
              )}
              <span className="work-record-duration">
                {step.duration_ms == null ? 'now' : formatDuration(step.duration_ms)}
              </span>
            </li>
          ))}
          {steps.data?.steps.length === 0 && (
            <li className="work-record-none">No tool ran in this turn.</li>
          )}
        </ol>
      )}
    </div>
  )
}
