// One API stub for the shell tests (App and routing): the two channels,
// one agent, an empty timeline, and the endpoints each route reads.

export function shellResponse(path: string): { data: unknown } {
  if (path === '/api/v1/settings/onboarding') {
    return { data: { completed: true, docker: { endpoint: 'unix:///var/run/docker.sock', candidates: [] }, docker_endpoint: null, providers: [] } }
  }
  if (path === '/api/v1/memory/pages') {
    return { data: { pages: [], next: null, total: 0 } }
  }
  if (path === '/api/v1/memory/pages/counts') {
    return { data: { pages: 0, procedures: 0, authors: [] } }
  }
  if (path === '/api/v1/channels') {
    return {
      data: {
        items: [
          {
            id: 'channel-1',
            workspace_id: 'workspace-1',
            kind: 'dm',
            title: 'Sage',
            agent_ids: ['agent-1'],
            user_member: true,
            created_at: 1,
            updated_at: 1,
          },
          {
            id: 'channel-2',
            workspace_id: 'workspace-1',
            kind: 'group',
            title: 'Launch planning',
            agent_ids: ['agent-1'],
            user_member: true,
            created_at: 2,
            updated_at: 2,
          },
        ],
      },
    }
  }
  if (path === '/api/v1/channels/{channel_id}/messages') {
    return { data: { items: [] } }
  }
  if (path === '/api/v1/channels/{channel_id}/threads/{root_message_id}') {
    return {
      data: {
        root: {
          id: 'message-1',
          channel_id: 'channel-2',
          author_kind: 'user',
          author_agent_id: null,
          created_at: 1,
          status: 'completed',
          run_id: null,
          blocks: [{ type: 'markdown', text: 'Ship the launch plan' }],
          text_content: 'Ship the launch plan',
        },
        replies: [],
      },
    }
  }
  if (path === '/api/v1/agents') {
    return {
      data: {
        items: [
          {
            id: 'agent-1',
            name: 'Sage',
            job: 'General assistant',
            personality: 'Direct',
            avatar: { sprite: 'pixie', preset: 'mint', colors: {}, accessories: {} },
            status: 'active',
          },
        ],
      },
    }
  }
  if (path === '/api/v1/workspace') {
    return {
      data: {
        id: 'workspace-1',
        name: 'Workspace',
        timezone: 'UTC',
        chief_of_staff_agent_id: 'agent-1',
      },
    }
  }
  if (path === '/api/v1/agents/{agent_id}/computer') {
    return { data: { state: 'off', percent: null, holder: 'agent' } }
  }
  if (path === '/api/v1/memory/feed') return { data: { items: [] } }
  if (path === '/api/v1/calls/{call_id}') {
    return {
      data: {
        id: 'call_1',
        agent_id: 'agent-1',
        agent_name: 'Sage',
        own_e164: '+14155550123',
        direction: 'outbound',
        remote_e164: '+14155559981',
        purpose: 'Ask about the table.',
        tools: [],
        tier: 'unknown',
        state: 'live',
        outcome: null,
        ended_reason: null,
        classification: null,
        message_left: false,
        transcript: [],
        recording_artifact_id: null,
        created_at: 0,
        answered_at: 1,
        ended_at: null,
      },
    }
  }
  return { data: { items: [] } }
}
