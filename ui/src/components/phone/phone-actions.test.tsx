import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory } from '@tanstack/react-router'
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, expect, it, vi } from 'vitest'
import { App } from '../../App'
import type { ApiClient, CallDto, RequestDto } from '../../api/client'
import { usePresence } from '../../state/presence'
import { useComposerDraft } from '../../state/composerDraft'
import { shellResponse } from '../../test/appStub'
import { callBackDraft } from '../home/queue'

const { api } = vi.hoisted(() => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
}))
vi.mock('../../api/client', async () => ({
  ...(await vi.importActual('../../api/client')),
  createApiClient: () => api as unknown as ApiClient,
}))
vi.mock('../../ws/socket', () => ({
  PagisSocket: class {
    start() {}
    stop() {}
    subscribeChannel() {}
    activity() {
      return false
    }
  },
}))

const approval = {
  id: 'request-1',
  agent_id: 'agent-1',
  kind: 'tool_action',
  state: 'pending',
  created_at: Date.now(),
  run_id: null,
  payload: {
    action_title: 'Send the invoice',
    tool_name: 'mail__send',
    arguments: {
      to: ['billing@example.com'],
      subject: 'October invoice',
      body: 'The invoice is attached.',
    },
    proposed_rules: ['example.com'],
  },
} as RequestDto
let request: RequestDto
let call: CallDto

beforeEach(() => {
  vi.stubGlobal('matchMedia', (query: string) => ({
    matches: query.includes('max-width'),
    media: query,
    addEventListener() {},
    removeEventListener() {},
  }))
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response('missing', { status: 404 })),
  )
  usePresence.setState({ runs: {}, onCall: {}, unread: {}, seeded: false })
  useComposerDraft.setState({ byScope: {} })
  request = structuredClone(approval)
  call = {
    ...(shellResponse('/api/v1/calls/{call_id}').data as CallDto),
    id: 'call-1',
    direction: 'inbound',
    state: 'ended',
    outcome: 'no_answer',
    message_left: true,
    transcript: [{ at: 1, speaker: 'caller', text: 'Please check invoice 1038.' }],
  }
  api.GET.mockReset()
  api.GET.mockImplementation(async (path: string) =>
    path === '/api/v1/requests/{request_id}'
      ? { data: request }
      : path === '/api/v1/calls/{call_id}'
        ? { data: call }
        : shellResponse(path),
  )
  api.POST.mockReset()
  api.POST.mockResolvedValue({ data: {}, response: new Response(null, { status: 204 }) })
})

function mount(path: string) {
  const history = createMemoryHistory({ initialEntries: [path] })
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <App history={history} />
    </QueryClientProvider>,
  )
  return history
}

it.each([true, false])('offers Google sign-in only when the installation is set up: %s', async (setUp) => {
  api.GET.mockImplementation(async (path: string) =>
    path === '/api/v1/settings/connections/providers'
      ? { data: { items: [{ id: 'google', kind: 'oauth', fields: [], set_up: setUp }] } }
      : shellResponse(path),
  )
  mount('/settings/connections')
  fireEvent.click(await screen.findByRole('button', { name: 'Add a connection' }))
  await screen.findByRole('dialog', { name: 'Add a connection' })
  if (setUp) {
    expect(await screen.findByRole('button', { name: 'Continue at Google' })).not.toBeNull()
  } else {
    expect(await screen.findByText('Google is not set up on this server yet.')).not.toBeNull()
    expect(screen.queryByRole('button', { name: 'Continue at Google' })).toBeNull()
  }
  expect(screen.queryByLabelText(/Client ID|Client secret|Google account address/)).toBeNull()
})

it.each(['approved', 'denied'] as const)(
  'uses the allow rule only for an approved decision: %s',
  async (decision) => {
    mount('/?request=request-1')
    await screen.findByText('October invoice')
    fireEvent.click(screen.getByRole('switch', { name: /Always allow/ }))
    fireEvent.click(
      screen.getByRole('button', { name: decision === 'approved' ? 'Approve' : 'Deny' }),
    )
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/requests/{request_id}/decision', {
        params: { path: { request_id: 'request-1' } },
        body: {
          decision,
          scope: decision === 'approved' ? 'always' : undefined,
          values: undefined,
        },
      }),
    )
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
  },
)

it('says the same Always allow words as the desktop card, and shows the email body', async () => {
  mount('/?request=request-1')
  const always = await screen.findByRole('switch', { name: /Always allow/ })
  expect(within(always).getByText('Always allow')).not.toBeNull()
  expect(within(always).getByText('example.com')).not.toBeNull()
  expect(screen.queryByText(/sends email to addresses at/)).toBeNull()
  // A mail rule is a recipient domain, not a command with flags.
  expect(screen.queryByText(/every flag and argument/)).toBeNull()
  const body = screen.getByText('The invoice is attached.')
  expect(body.closest('summary')).toBeNull()
})

it.each([
  ['execute', true],
  ['edit', false],
])(
  'warns about every flag and argument for a Harness Permission of kind %s: %s',
  async (toolKind, warns) => {
    request = {
      ...approval,
      kind: 'harness_permission',
      payload: {
        action_title: 'Claude Code wants to run a command',
        harness_name: 'Claude Code',
        host_name: 'Air',
        directory: '/work/pagis',
        tool_kind: toolKind,
        command: 'cargo test',
        proposed_rules: ['cargo test'],
      },
    } as RequestDto
    mount('/?request=request-1')
    const always = await screen.findByRole('switch', { name: /Always allow/ })
    expect(within(always).getByText('Always allow')).not.toBeNull()
    expect(within(always).getByText('cargo test')).not.toBeNull()
    expect(screen.queryByText(/every flag and argument/) !== null).toBe(warns)
  },
)

it('keeps a settled approval read only', async () => {
  request.state = 'expired'
  mount('/?request=request-1')
  expect(await screen.findByText('Expired')).not.toBeNull()
  expect(screen.queryByRole('button', { name: 'Approve' })).toBeNull()
  expect(screen.queryByRole('switch')).toBeNull()
})

it('opens the approval conversation with the composer focused', async () => {
  const history = mount('/?request=request-1')
  fireEvent.click(await screen.findByRole('button', { name: 'Reply to Sage instead' }))
  await waitFor(() => expect(history.location.pathname).toBe('/c/channel-1'))
  await waitFor(() =>
    expect(document.activeElement).toBe(document.querySelector('.composer-input')),
  )
  expect(api.POST).not.toHaveBeenCalled()
})

it('dismisses a missed call and puts the displayed callback draft in its conversation', async () => {
  const history = mount('/calls/call-1')
  const draft = callBackDraft({ remote_e164: call.remote_e164, left_message: call.message_left })
  expect(await screen.findByText(draft)).not.toBeNull()
  expect(screen.getByText('Please check invoice 1038.')).not.toBeNull()
  fireEvent.click(screen.getByRole('button', { name: 'Call back' }))
  await waitFor(() =>
    expect(api.POST).toHaveBeenCalledWith('/api/v1/calls/{call_id}/dismiss', {
      params: { path: { call_id: 'call-1' } },
    }),
  )
  await waitFor(() => expect(history.location.pathname).toBe('/c/channel-1'))
  await waitFor(() =>
    expect(document.querySelector<HTMLTextAreaElement>('.composer-input')?.value).toBe(draft),
  )
})

it('shows the no-message state and dismisses it to Home', async () => {
  call.message_left = false
  call.transcript = []
  const history = mount('/calls/call-1')
  expect(await screen.findByText('Nobody answered.')).not.toBeNull()
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }))
  await waitFor(() => expect(history.location.pathname).toBe('/'))
})

it.each([
  ['Hang up', '/api/v1/calls/{call_id}/hangup'],
  ['Drop to Unknown', '/api/v1/calls/{call_id}/tier'],
])('confirms %s before changing a live call', async (label, endpoint) => {
  call.state = 'live'
  call.tier = 'trusted'
  mount('/calls/call-1')
  fireEvent.click(await screen.findByRole('button', { name: label }))
  expect(api.POST).not.toHaveBeenCalled()
  const confirm = screen.getByRole('alertdialog')
  expect(document.activeElement).toBe(within(confirm).getByRole('button', { name: 'Cancel' }))
  fireEvent.click(within(confirm).getByRole('button', { name: label }))
  await waitFor(() =>
    expect(api.POST).toHaveBeenCalledWith(
      endpoint,
      expect.objectContaining({ params: { path: { call_id: 'call-1' } } }),
    ),
  )
})

it('offers no tier change for an Unknown live call', async () => {
  call.state = 'live'
  mount('/calls/call-1')
  await screen.findByText('Call in progress')
  expect(screen.queryByRole('button', { name: 'Drop to Unknown' })).toBeNull()
})
