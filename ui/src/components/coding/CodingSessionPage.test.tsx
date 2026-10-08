// `/coding/:sessionId` shows one Coding Session: the head with its
// facts, the plan as a checklist, and the transcript as messages, tool
// calls and lines.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, CodingSessionEventDto } from '../../api/client'
import { codingSession } from '../../test/appStub'
import { renderInRouter } from '../../test/router'
import { CodingSessionPage } from './CodingSessionPage'

function row(
  seq: number,
  kind: CodingSessionEventDto['kind'],
  payload: Record<string, unknown>,
): CodingSessionEventDto {
  return { seq, at: seq, kind, payload }
}

const rows: CodingSessionEventDto[] = [
  row(1, 'prompt', { text: 'Fix the login bug', message_id: null }),
  row(2, 'plan', {
    entries: [
      { content: 'Read the login code', priority: 'medium', status: 'completed' },
      { content: 'Fix the token check', priority: 'high', status: 'in_progress' },
      { content: 'Run the tests', priority: 'low', status: 'pending' },
    ],
  }),
  row(3, 'tool_call', {
    toolCallId: 'call-1',
    title: 'Run cargo test',
    kind: 'execute',
    status: 'pending',
    locations: [{ path: '/Users/ada/src/app/Cargo.toml' }],
    rawInput: { command: 'cargo test' },
  }),
  row(4, 'tool_call_update', {
    toolCallId: 'call-1',
    status: 'completed',
    content: [
      { type: 'content', content: { type: 'text', text: 'test result: ok. 12 passed' } },
      { type: 'diff', path: '/Users/ada/src/app/src/login.rs', oldText: 'a', newText: 'b' },
    ],
  }),
  row(5, 'agent_message', {
    text: 'Done. <script>window.injected = true</script> **The tests pass.**',
    message_id: 'm1',
  }),
  row(6, 'turn_end', { stop_reason: 'end_turn' }),
]

function stubApi({
  session = codingSession as unknown,
  transcript = rows,
}: { session?: unknown; transcript?: CodingSessionEventDto[] } = {}) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/coding-sessions/{coding_session_id}') {
        return session === null
          ? { error: { error: { code: 'not_found', message: 'no such session' } } }
          : { data: session }
      }
      if (path === '/api/v1/coding-sessions/{coding_session_id}/transcript') {
        return { data: { items: transcript, next_after: null } }
      }
      if (path === '/api/v1/agents') {
        return { data: { items: [{ id: 'agent-1', name: 'Sage', job: 'a', status: 'active' }] } }
      }
      return { data: { items: [] } }
    }),
  } as unknown as ApiClient
}

function mount(api = stubApi()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return renderInRouter(
    <QueryClientProvider client={client}>
      <CodingSessionPage api={api} sessionId="session-1" />
    </QueryClientProvider>,
  )
}

describe('the head', () => {
  it('shows the sprite, the harness and the facts of the session', async () => {
    mount()

    const title = await screen.findByRole('heading', { name: 'Fix the login bug' })
    const head = title.closest('header') as HTMLElement
    expect(await within(head).findByText('Sage')).toBeTruthy()
    expect(within(head).getByText('Claude Code')).toBeTruthy()
    expect(within(head).getByText('Ada’s laptop')).toBeTruthy()
    expect(within(head).getByText('/Users/ada/src/app')).toBeTruthy()
    expect(within(head).getByText('pagis/fix-login')).toBeTruthy()
    expect(within(head).getByText('Running')).toBeTruthy()
    expect(within(head).getByText('You approve')).toBeTruthy()
    expect(within(head).getByText('38% of context')).toBeTruthy()
  })

  it('names the sprite in the mode that lets it approve', async () => {
    mount(stubApi({ session: { ...codingSession, approval_mode: 'agent' } }))

    expect(await screen.findByText('Sage approves')).toBeTruthy()
  })

  it('opens the Thread of the session from its link', async () => {
    const history = mount()

    fireEvent.click(await screen.findByRole('link', { name: 'Open the conversation' }))

    expect(await screen.findByTestId('thread-view')).toBeTruthy()
    expect(history.location.pathname).toBe('/c/channel-2/t/message-1')
  })
})

describe('the plan', () => {
  it('shows each entry with its status, and marks a high priority', async () => {
    mount()

    const plan = await screen.findByRole('region', { name: 'Plan' })
    const entries = within(plan).getAllByRole('listitem')
    expect(entries.map((entry) => entry.textContent)).toEqual([
      'Read the login codeDone',
      'Fix the token checkHigh priorityIn progress',
      'Run the testsNot started',
    ])
  })

  it('is absent when the harness made no plan', async () => {
    mount(stubApi({ transcript: [rows[0]] }))

    await screen.findByText('Fix the login bug', { selector: 'p' })
    expect(screen.queryByRole('region', { name: 'Plan' })).toBeNull()
  })
})

describe('the transcript', () => {
  it('shows a tool call with its kind, its title, its status and its locations', async () => {
    mount()

    const tool = await screen.findByRole('article', { name: 'Run cargo test' })
    expect(within(tool).getByText('Command')).toBeTruthy()
    expect(within(tool).getByText('Done')).toBeTruthy()
    expect(within(tool).getByText('/Users/ada/src/app/Cargo.toml')).toBeTruthy()
    expect(within(tool).getByText('test result: ok. 12 passed')).toBeTruthy()
    expect(within(tool).getByText('/Users/ada/src/app/src/login.rs')).toBeTruthy()
  })

  it('keeps the input of a tool call behind a disclosure', async () => {
    mount()

    const tool = await screen.findByRole('article', { name: 'Run cargo test' })
    expect(within(tool).queryByText(/"command": "cargo test"/)).toBeNull()

    fireEvent.click(within(tool).getByRole('button', { name: 'Show the input' }))

    expect(within(tool).getByText(/"command": "cargo test"/)).toBeTruthy()
  })

  // Harness output is foreign text: it draws through the prose
  // renderer, which keeps raw HTML off.
  it('shows HTML in a message of the harness as text', async () => {
    mount()

    expect(await screen.findByText('The tests pass.')).toBeTruthy()
    expect(document.querySelector('script')).toBeNull()
    expect((window as { injected?: boolean }).injected).toBeUndefined()
  })

  it('shows the end of a turn and the message of the sprite', async () => {
    mount()

    expect(await screen.findByText('The turn ended')).toBeTruthy()
    expect(screen.getByText('Fix the login bug', { selector: 'p' })).toBeTruthy()
  })

  it('shows a permission and its decision as one line', async () => {
    mount(
      stubApi({
        transcript: [
          row(1, 'permission', {
            ask_id: '7',
            tool_call_id: 'call-1',
            title: 'Edit src/login.rs',
            kind: 'edit',
            locations: [],
            options: ['allow_once'],
            waits_for: null,
          }),
          row(2, 'decision', { ask_id: '7', decision: 'allow_once', decider: 'scope' }),
        ],
      }),
    )

    expect(await screen.findByText('Edit src/login.rs')).toBeTruthy()
    expect(screen.getByText("Allowed: inside the session's directory")).toBeTruthy()
  })

  it('marks a row that the daemon cut', async () => {
    mount(
      stubApi({
        transcript: [
          row(1, 'tool_call', {
            toolCallId: 'call-1',
            title: 'Read a large file',
            kind: 'read',
            status: 'completed',
            truncated: true,
          }),
        ],
      }),
    )

    const tool = await screen.findByRole('article', { name: 'Read a large file' })
    expect(within(tool).getByText('Cut at 16 KB')).toBeTruthy()
  })
})

describe('a session that is not there', () => {
  it('says that it is not on record', async () => {
    mount(stubApi({ session: null }))

    await waitFor(() =>
      expect(screen.getByText('That coding session is not on record.')).toBeTruthy(),
    )
  })
})
