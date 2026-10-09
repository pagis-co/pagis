// The running transcript of one Call. The settled block and the
// call inspector read the same lines, so they draw them the same way.
// A line the daemon wrote is what happened and nobody said: the tier,
// the keypad, the classify verdict.

import type { CallLine } from '../state/stores'
import { speakerLabel } from './call'

import './call.css'

function clockOf(at: number): string {
  return new Date(at).toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit',
  })
}

export function Transcript({
  lines,
  agentName,
  callerLabel,
}: {
  lines: readonly CallLine[]
  agentName: string
  callerLabel?: string
}) {
  if (lines.length === 0) {
    return <p className="call-transcript-empty">Nothing has been said yet.</p>
  }
  return (
    <ol className="call-transcript" data-testid="call-transcript">
      {lines.map((line, index) => (
        <li key={index} className={`call-line call-line-${line.speaker}`}>
          <span className="call-line-at">{clockOf(line.at)}</span>
          <span className="call-line-who">{line.speaker === 'caller' && callerLabel ? callerLabel : speakerLabel(line.speaker, agentName)}</span>
          <span className="call-line-text">{line.text}</span>
        </li>
      ))}
    </ol>
  )
}
