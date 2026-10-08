// The daemon a connection page test talks to: one Google
// connection, one agent, the sync of the account and its catalogue.
// Every stub is cast to the client, so the shape is the test's own.

import { vi } from 'vitest'

import type { ApiClient, ConnectionDto } from '../../api/client'
import type { Catalogue, ReflectionFilter } from './rules'

export const catalogue: { catalogue: Catalogue; default_filter: ReflectionFilter } = {
  catalogue: {
    signals: [
      { id: 'subject', label: 'Subject', kind: { kind: 'text' } },
      { id: 'sender_address', label: 'Sender address', kind: { kind: 'list', format: 'email' } },
      { id: 'messages', label: 'Messages', kind: { kind: 'number' } },
      { id: 'labels', label: 'Labels', kind: { kind: 'tag_set', options: ['IMPORTANT', 'STARRED'] } },
    ],
  },
  default_filter: {
    rules: [{
      verdict: 'reflect',
      conditions: [{ signal: 'labels', operator: 'has', value: { kind: 'choice', value: 'IMPORTANT' } }],
    }],
    default: 'reflect',
  } as ReflectionFilter,
}

export const filter: ReflectionFilter = {
  rules: [
    {
      verdict: 'reflect',
      conditions: [{ signal: 'labels', operator: 'has', value: { kind: 'choice', value: 'IMPORTANT' } }],
    },
    {
      verdict: 'skip',
      conditions: [
        { signal: 'sender_address', operator: 'in', value: { kind: 'list', values: ['noreply@x.io'] } },
        { signal: 'messages', operator: 'at_most', value: { kind: 'number', value: 1 } },
      ],
    },
  ],
  default: 'skip',
}

export const status = {
  config: {
    workspace_id: 'w', connection_id: 'conn-1', resource: 'gmail', agent_id: 'ag1',
    required_capability: 'gmail_read', enabled: true, since: Date.UTC(2025, 7, 9),
    filter,
  },
  revision: 1, cursor_revision: 1, checkpoint: null, caught_up: true, rebuilding: false,
  processed: 12480, arrival_pending: 0, arrival_processed: 41, arrival_skipped: 2,
  filter_revision: 1, reflected_pages: 2, skipped_pages: 5,
  backfill_pending: 240, backfill_reflected: 940, backfill_failed: 0, backfill_capped: false,
  incomplete_versions: 0, acquisition_error: null, arrival_error: null,
  updated_at: Date.now() - 3 * 60_000,
}

export const preview = { total: 3412, reflect: 1180, skip: 2232, rules: [1000, 180] }

export const forgetPreview = {
  revision: 'sql-1', memory_revision: 'git-1', source_items: 2,
  memory_paths: 2, memory_revisions: 4, raw_account_retrieval_blocked: true,
}

export function connected(overrides: Partial<ConnectionDto> = {}): ConnectionDto {
  return {
    id: 'conn-1',
    provider: 'google',
    capabilities: ['mail', 'calendar'],
    absent_capabilities: [],
    installation: false,
    alias: 'personal',
    display_name: 'Google',
    status: 'connected',
    auth_mode: 'byo',
    account: 'alice@example.com',
    authorized_capabilities: ['gmail_read', 'gmail_send', 'calendar_read'],
    created_at: 1,
    ...overrides,
  }
}

export const sage = { id: 'ag1', name: 'Sage', job: 'assistant', personality: 'warm', status: 'active' }
export const clown = { id: 'ag2', name: 'Clown', job: 'jester', personality: 'loud', status: 'active' }

export const sageGrant = {
  id: 'grant-1', agent_id: 'ag1', agent_name: 'Sage', resource_kind: 'connection',
  resource_id: 'conn-1', allow: [], capabilities: ['gmail_read', 'gmail_send', 'calendar_read'], sessions: [],
  revision: 1, created_at: 1,
}

export function stubApi({
  connections = [connected()],
  agents = [sage, clown],
  grants = [sageGrant],
  sync = status as unknown,
  operations = [] as unknown[],
} = {}) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/settings/connections') return { data: { items: connections } }
      if (path === '/api/v1/agents') return { data: { items: agents } }
      if (path === '/api/v1/grants') return { data: { items: grants } }
      if (path.endsWith('/sync')) return { data: { status: sync, blocked_reason: null } }
      if (path.endsWith('/catalogue')) return { data: catalogue }
      if (path === '/api/v1/knowledge/forget') return { data: operations }
      return { data: { items: [] } }
    }),
    POST: vi.fn(async (path: string) => {
      if (path.endsWith('/filter/preview')) return { data: preview }
      if (path === '/api/v1/knowledge/forget/preview') return { data: forgetPreview }
      if (path.endsWith('/authorize')) return { data: connected() }
      if (path === '/api/v1/grants') return { data: sageGrant }
      return { data: {} }
    }),
    PUT: vi.fn(async () => ({ data: { status: sync, blocked_reason: null } })),
    DELETE: vi.fn(async () => ({ error: undefined, response: { ok: true } })),
  }
}

export type StubApi = ReturnType<typeof stubApi>

export function asClient(api: StubApi): ApiClient {
  return api as unknown as ApiClient
}
