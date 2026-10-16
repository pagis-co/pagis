import { useState } from 'react'
import type { AgentDto, ApiClient } from '../../api/client'
import { Button, Textarea } from '../../primitives'
import { errorMessage, useUpdateAgent } from '../../queries'

export function StandingBrief({ api, agent }: { api: ApiClient; agent: AgentDto }) {
  const update = useUpdateAgent(api)
  const saved = agent.standing_brief ?? ''
  const [brief, setBrief] = useState(saved)
  const changed = brief.trim() !== saved.trim()

  return (
    <div className="phone-brief">
      <label className="settings-label" htmlFor="standing-brief">
        Standing Brief for the Chief of Staff, {agent.name}
      </label>
      <p className="settings-hint">
        {agent.name} reads this at the start of every call it answers.
        Leave it empty and it takes a message: who called, what they
        need, and how to reach you.
      </p>
      <Textarea
        id="standing-brief"
        aria-label={`Standing Brief for the Chief of Staff, ${agent.name}`}
        rows={4}
        value={brief}
        placeholder="For example: say I am in meetings until the evening, and ask what the call is about."
        onChange={(e) => setBrief(e.target.value)}
      />
      {update.error != null && (
        <p className="settings-error" role="alert">
          {errorMessage(update.error, 'That brief could not be saved.')}
        </p>
      )}
      <div className="settings-row-actions">
        <Button
          variant="primary"
          disabled={!changed || update.isPending}
          onClick={() =>
            update.mutate({
              agentId: agent.id,
              name: agent.name,
              job: agent.job,
              description: agent.description,
              personality: agent.personality,
              voice: agent.voice ?? null,
              standing_brief: brief.trim() === '' ? null : brief,
            })
          }
        >
          {update.isPending ? 'Saving…' : 'Save the brief'}
        </Button>
        {changed && !update.isPending && (
          <Button type="button" onClick={() => setBrief(saved)}>
            Cancel
          </Button>
        )}
      </div>
    </div>
  )
}
