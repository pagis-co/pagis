// The `coding_session` block (ADR-0033). The daemon posts one in the
// Thread of the session when an Agent starts it.
//
// One card, running or settled, as the call block is (ADR-0022): the
// sprite's face, the harness and the title; where the session runs; the
// state and the mode; where a decision waits; the last line of activity;
// and the end of a settled session. "Open" goes to the session page, and
// Stop closes a session that runs.
//
// Every fact comes from the session record. A `coding_session.*` frame
// only makes the block read it again (`AppShell`). The block does not
// draw the approval card of a Harness Permission: that card is its own
// message in the Thread, and the block only says where the decision
// waits. The last line of activity is harness text, so it shows as plain
// text and never as Markdown.

import { Link } from '@tanstack/react-router'
import { GitBranch } from 'lucide-react'

import type { ApiClient } from '../api/client'
import { StopCodingSession } from '../components/coding/StopCodingSession'
import {
  approvalModeBadge,
  endReasonText,
  pendingText,
  sessionSettled,
  sessionStateBadge,
  usageText,
} from '../components/coding/words'
import { Avatar, Badge } from '../primitives'
import { useAgents, useCodingSession } from '../queries'
import { formatMoment } from '../timeline'
import { Card, CardBody, CardFooter } from './Card'

import './codingSession.css'

export function CodingSessionBlock({ sessionId, api }: { sessionId: string; api: ApiClient }) {
  const session = useCodingSession(api, sessionId)
  const agents = useAgents(api)
  if (session.data === undefined) {
    return (
      <Card className="coding-block">
        <CardBody className="coding-block-waiting">
          {session.isError ? 'That coding session is not on record.' : 'Opening the coding session…'}
        </CardBody>
      </Card>
    )
  }

  const record = session.data
  const agent = agents.data?.find((candidate) => candidate.id === record.agent_id)
  const spriteName = agent?.name ?? 'A sprite'
  const settled = sessionSettled(record.state)
  const state = sessionStateBadge(record.state)
  const mode = approvalModeBadge(record.approval_mode, spriteName)
  const usage = usageText(record.usage)
  // A session in the Agent's Computer has no Host name.
  const machine = record.machine_name ?? 'Computer'
  return (
    <Card className="coding-block" data-testid="coding-session-block">
      <div className="block-card-header">
        {/* No presence ring: the ring is the Agent's Activity, and the
            state badge says what the harness does. */}
        <Avatar id={record.agent_id} name={spriteName} appearance={agent?.avatar} size="sm" />
        <span className="block-card-title">
          <strong>{record.harness_name}</strong>
          <span className="block-card-place">{record.title}</span>
        </span>
      </div>
      <CardBody className="coding-block-body">
        <ul className="coding-block-facts">
          <li>{machine}</li>
          <li className="coding-block-path" title={record.directory}>
            {record.directory}
          </li>
          {record.worktree_branch != null && (
            <li className="coding-block-branch">
              <GitBranch size={14} aria-hidden focusable="false" />
              <span className="coding-block-path">{record.worktree_branch}</span>
            </li>
          )}
        </ul>
        <p className="coding-block-status">
          <Badge tone={state.tone}>{state.label}</Badge>
          <Badge tone={mode.tone}>{mode.label}</Badge>
          {usage !== null && <span className="coding-block-usage">{usage}</span>}
        </p>
        {record.state === 'needs_decision' && record.pending != null && (
          <p className="coding-block-pending">
            {pendingText(record.pending, spriteName, record.harness_name)}
          </p>
        )}
        {record.last_activity != null && record.last_activity !== '' && (
          <p className="coding-block-activity">{record.last_activity}</p>
        )}
        {settled && (
          <p className="coding-block-end">
            <span>{endReasonText(record.end_reason, spriteName)}</span>
            {record.ended_at != null && (
              <>
                <span aria-hidden> · </span>
                <span>{formatMoment(record.ended_at)}</span>
              </>
            )}
          </p>
        )}
      </CardBody>
      <CardFooter className="coding-block-footer">
        <Link
          to="/coding/$sessionId"
          params={{ sessionId: record.id }}
          className="coding-block-open"
        >
          Open
        </Link>
        {!settled && <StopCodingSession api={api} sessionId={record.id} />}
      </CardFooter>
    </Card>
  )
}
