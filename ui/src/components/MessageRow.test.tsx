// One message of the Thread: your message is a bubble at the
// right with the time under it, the sprite speaks in prose under a name
// line, and the hover tools carry the acts on one message. The Run link
// is a router Link, so the work record opens without a reload.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen } from '@testing-library/react'
import type { ReactNode } from 'react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { useSpeaking } from '../state/stores'
import { renderInRouter } from '../test/router'
import { formatClock, type TimelineRow } from '../timeline'
import { MessageRow } from './MessageRow'

// The rows under test render markdown blocks only, so the API client
// is never called.
const api = {} as ApiClient

function row(overrides: Partial<TimelineRow>): TimelineRow {
  return {
    key: 'msg-1',
    kind: 'message',
    authorKind: 'user',
    authorAgentId: null,
    createdAt: Date.parse('2026-09-05T14:58:00Z'),
    sendState: 'sent',
    status: 'complete',
    runId: null,
    blocks: [{ type: 'markdown', text: 'hi' }],
    text: 'hi',
    completedAt: null,
    replyCount: 0,
    lastReplyAt: null,
    replyAuthors: [],
    ...overrides,
  }
}

function mount(
  r: TimelineRow,
  props: Partial<Parameters<typeof MessageRow>[0]> = {},
) {
  return render(<MessageRow api={api} row={r} onRetry={() => {}} {...props} />)
}

/** A reply from a Run reads the memory feed, so it needs a query client. */
function withQueries(ui: ReactNode) {
  return <QueryClientProvider client={new QueryClient()}>{ui}</QueryClientProvider>
}

const agentRow = (overrides: Partial<TimelineRow> = {}) =>
  row({ authorKind: 'agent', authorAgentId: 'ag-1', ...overrides })

describe('MessageRow', () => {
  it('draws your message as a bubble with the time under it', () => {
    const at = row({})
    mount(at)

    const message = screen.getByTestId('message-row')
    expect(message.className).toContain('message-mine')
    expect(message.querySelector('.message-bubble')?.textContent).toBe('hi')
    expect(
      screen.getByText(formatClock(at.createdAt)),
    ).toBeTruthy()
  })

  it('leaves the time off a message that follows yours', () => {
    const at = row({})
    mount(at, { grouped: true })

    expect(
      screen.queryByText(formatClock(at.createdAt)),
    ).toBeNull()
  })

  it('says a send is on its way, and retries one that failed', () => {
    mount(row({ sendState: 'pending' }))
    expect(screen.getByText('sending…')).toBeTruthy()

    const onRetry = vi.fn()
    mount(row({ key: 'p-2', sendState: 'failed' }), { onRetry })
    fireEvent.click(screen.getByText('Failed — retry'))
    expect(onRetry).toHaveBeenCalledWith('p-2', 'hi')
  })

  it('writes the sprite in prose under a name line, with no bubble', () => {
    mount(agentRow(), { authorName: 'Scout' })

    const message = screen.getByTestId('message-row')
    expect(screen.getByText('Scout')).toBeTruthy()
    expect(message.querySelector('.message-bubble')).toBeNull()
    expect(message.querySelector('.message-prose')?.textContent).toBe('hi')
  })

  it('falls back to the kind when the agent name is unknown', () => {
    mount(agentRow())

    expect(screen.getByText('Sprite')).toBeTruthy()
  })

  it('draws no name line on a grouped row', () => {
    mount(agentRow(), { authorName: 'Scout', grouped: true })

    expect(screen.queryByText('Scout')).toBeNull()
    expect(screen.getByTestId('message-row').dataset.grouped).toBe('true')
  })

  it('marks a reply this session read aloud', () => {
    useSpeaking.setState({ byScope: {}, spoken: { 'msg-1': true } })
    mount(agentRow())
    expect(screen.getByText('spoken')).toBeTruthy()

    useSpeaking.setState({ byScope: {}, spoken: {} })
    mount(agentRow({ key: 'msg-2' }))
    expect(screen.queryByText('spoken')).toBeNull()
  })

  it('folds the work record under a reply that came from a Run', async () => {
    const steps = { run_id: 'run-1', state: 'completed', worked_ms: 9_000, stopped: null, failure: null, steps: [] }
    const client = { GET: vi.fn(async () => ({ data: steps })) }
    renderInRouter(
      <QueryClientProvider client={new QueryClient()}>
        <MessageRow
          api={client as unknown as ApiClient}
          row={agentRow({ runId: 'run-1' })}
          work={{ runId: 'run-1', startedAt: 0, endedAt: 9_000, outcome: 'Done' }}
          onRetry={() => {}}
        />
      </QueryClientProvider>,
    )

    expect(await screen.findByTestId('work-record')).toBeTruthy()
  })

  it('puts the learned line under a reply whose Run wrote memory', async () => {
    const feed = {
      revision: 'head1',
      items: [
        {
          id: 'e1',
          kind: 'committed',
          sha: 'abc123',
          agent_id: 'ag-1',
          agent_name: 'Sage',
          scopes: ['shared'],
          files: ['shared/MEMORY.md'],
          titles: ['Shared memory'],
          message: 'Noted the tea preference.',
          run_id: 'run-1',
          message_id: 'msg-1',
          source_scoped: false,
          reverted_sha: null,
          created_at: 0,
          action: null,
        },
      ],
    }
    const client = { GET: vi.fn(async () => ({ data: feed })) }
    renderInRouter(
      withQueries(
        <MessageRow
          api={client as unknown as ApiClient}
          row={agentRow({ runId: 'run-1' })}
          authorName="Sage"
          onRetry={() => {}}
        />,
      ),
    )

    const line = await screen.findByTestId('learned-line')
    expect(line.textContent).toContain('Sage learned · noted the tea preference on Shared memory')
  })

  it('closes the reply with the chip, under what the Run learned', async () => {
    const feed = {
      revision: 'head1',
      items: [
        {
          id: 'e1',
          kind: 'committed',
          sha: 'abc123',
          agent_id: 'ag-1',
          agent_name: 'Sage',
          scopes: ['shared'],
          files: ['shared/MEMORY.md'],
          titles: ['Shared memory'],
          message: 'Noted the tea preference.',
          run_id: 'run-1',
          message_id: 'msg-1',
          source_scoped: false,
          reverted_sha: null,
          created_at: 0,
          action: null,
        },
      ],
    }
    const client = { GET: vi.fn(async () => ({ data: feed })) }
    renderInRouter(
      withQueries(
        <MessageRow
          api={client as unknown as ApiClient}
          row={agentRow({
            runId: 'run-1',
            replyCount: 2,
            lastReplyAt: 5000,
            replyAuthors: [{ authorKind: 'user', agentId: null }],
          })}
          authorName="Sage"
          onRetry={() => {}}
          onOpenThread={() => {}}
        />,
      ),
    )

    const learned = await screen.findByTestId('learned-line')
    const chip = screen.getByRole('button', { name: /^2 replies/ })
    // `DOCUMENT_POSITION_FOLLOWING` is 4: the chip comes after the line.
    expect(learned.compareDocumentPosition(chip) & 4).toBe(4)
  })

  it('copies on a page with no Clipboard API through the copy command', async () => {
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined })
    const copied: string[] = []
    const execCommand = vi.fn((command: string) => {
      const area = document.querySelector<HTMLTextAreaElement>('textarea[readonly]')
      if (command === 'copy' && area !== null) copied.push(area.value)
      return true
    })
    Object.defineProperty(document, 'execCommand', { configurable: true, value: execCommand })
    renderInRouter(
      withQueries(<MessageRow api={api} row={agentRow({ runId: 'run-1' })} onRetry={() => {}} />),
    )

    fireEvent.click(await screen.findByLabelText('Copy'))

    expect(copied).toEqual(['hi'])
    expect(document.querySelector('textarea[readonly]')).toBeNull()
  })

  it('carries the hover tools: reply in thread, copy, read aloud, open the Run', async () => {
    const written: string[] = []
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: (text: string) => (written.push(text), Promise.resolve()) },
    })
    const onSpeak = vi.fn()
    const onOpenThread = vi.fn()
    renderInRouter(
      withQueries(
        <MessageRow
          api={api}
          row={agentRow({ runId: 'run-1' })}
          onRetry={() => {}}
          onSpeak={onSpeak}
          onOpenThread={onOpenThread}
        />,
      ),
    )

    fireEvent.click(await screen.findByLabelText('Reply in thread'))
    expect(onOpenThread).toHaveBeenCalledWith('msg-1')

    fireEvent.click(screen.getByLabelText('Copy'))
    expect(written).toEqual(['hi'])

    fireEvent.click(screen.getByLabelText('Read aloud'))
    expect(onSpeak).toHaveBeenCalledWith('msg-1')

    expect(screen.getByLabelText('Open the Run').getAttribute('href')).toBe(
      '/runs/run-1',
    )
  })

  // The work record is a URL, and the router owns it: the
  // link moves the router and the run view renders in the same
  // document. A plain anchor would reload the application instead.
  it('opens the Run through the router, without a full load', async () => {
    const history = renderInRouter(
      withQueries(
        <MessageRow api={api} row={agentRow({ runId: 'run-1' })} onRetry={() => {}} />,
      ),
    )
    const link = await screen.findByLabelText('Open the Run')

    const click = new MouseEvent('click', { bubbles: true, cancelable: true })
    fireEvent(link, click)

    expect(click.defaultPrevented).toBe(true)
    expect(await screen.findByTestId('run-view')).toBeTruthy()
    expect(history.location.pathname).toBe('/runs/run-1')
  })

  it('offers no reply in thread on a pending row or a thread reply', () => {
    mount(row({ sendState: 'pending' }), { onOpenThread: vi.fn() })
    expect(screen.queryByLabelText('Reply in thread')).toBeNull()

    // No handler (the thread pane's own rows): no affordance.
    mount(row({}))
    expect(screen.queryByLabelText('Reply in thread')).toBeNull()
  })
})

// The arrival fade. Only a row that lands while the reader
// watches fades up; a row the virtualizer mounts again, because the
// reader moved through the history, is old and appears at once.
describe('MessageRow arrival', () => {
  it('marks a message that just landed as fresh', () => {
    mount(row({ createdAt: Date.now() }))

    expect(
      screen.getByTestId('message-row').className.includes('message-fresh'),
    ).toBe(true)
  })

  it('leaves an older message unmarked', () => {
    mount(row({ createdAt: Date.now() - 60_000 }))

    expect(
      screen.getByTestId('message-row').className.includes('message-fresh'),
    ).toBe(false)
  })
})

// The replies chip: a message with replies in the channel
// timeline shows the chip. The thread pane passes no `onOpenThread`,
// so its rows show none.
describe('MessageRow replies', () => {
  const replied = { replyCount: 2, lastReplyAt: 5000, replyAuthors: [{ authorKind: 'user', agentId: null }] }
  const api = {
    GET: vi.fn(async () => ({ data: { items: [] } })),
  } as unknown as ApiClient
  const withQueries = (ui: ReactNode) =>
    render(
      <QueryClientProvider client={new QueryClient()}>{ui}</QueryClientProvider>,
    )

  it('shows the chip under an agent reply and under your message', () => {
    const onOpenThread = vi.fn()
    withQueries(
      <>
        <MessageRow api={api} row={agentRow(replied)} onRetry={() => {}} onOpenThread={onOpenThread} />
        <MessageRow api={api} row={row({ key: 'msg-2', ...replied })} onRetry={() => {}} onOpenThread={onOpenThread} />
      </>,
    )

    const chips = screen.getAllByRole('button', { name: /^2 replies/ })
    expect(chips).toHaveLength(2)
    fireEvent.click(chips[1])
    expect(onOpenThread).toHaveBeenCalledWith('msg-2')
  })

  it('shows no chip without replies or without a thread to open', () => {
    withQueries(
      <>
        <MessageRow api={api} row={agentRow()} onRetry={() => {}} onOpenThread={vi.fn()} />
        <MessageRow api={api} row={agentRow({ key: 'msg-2', ...replied })} onRetry={() => {}} />
      </>,
    )

    expect(screen.queryByRole('button', { name: /^\d+ repl/ })).toBeNull()
  })
})
