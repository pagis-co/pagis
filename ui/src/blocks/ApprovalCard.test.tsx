// The approval card: renders from the approval
// row (never from the block), Approve is the one primary action and
// "Always allow" is the checkbox that widens it, and a decided approval
// collapses to one line with the decision and the time. A credential
// action names the site, the user and the address it opens.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { ApiClient, MessageDto } from '../api/client'
import { Blocks } from './BlockView'

function stubApi(
  state: string,
  proposedRules: string[] = ['echo'],
  decidedAt: number | null = null,
) {
  return {
    GET: vi.fn(async () => ({
      data: {
        id: 'ap1',
        agent_id: 'ag1',
        run_id: 'r1',
        kind: 'host_action',
        payload: {
          command: 'echo hi',
          subcommands: ['echo hi'],
          proposed_rules: proposedRules,
        },
        state,
        decided_at: decidedAt,
        created_at: 1,
      },
    })),
    POST: vi.fn(async () => ({ data: { state: 'approved' } })),
  }
}

const cardBlock = {
  type: 'approval_card',
  request_id: 'ap1',
  title: 'Run a command on your computer',
  body: 'echo hi',
}

function mount(api: ReturnType<typeof stubApi>, blocks: unknown = [cardBlock]) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return render(
    <QueryClientProvider client={queryClient}>
      <Blocks
        blocks={blocks as MessageDto['blocks']}
        api={api as unknown as ApiClient}
      />
    </QueryClientProvider>,
  )
}

describe('ApprovalCard', () => {
  it('renders the denormalized title and body with the pending actions', async () => {
    const api = stubApi('pending')
    mount(api)

    expect(screen.getByText('Run a command on your computer')).toBeTruthy()
    expect(screen.getByText('echo hi')).toBeTruthy()
    // The state comes from the row: the buttons appear once it loads.
    expect(await screen.findByText('Approve')).toBeTruthy()
    expect(screen.getByText('Deny')).toBeTruthy()
    // One card, one primary action.
    expect(
      document.querySelectorAll('.approval-card .ui-button-primary'),
    ).toHaveLength(1)
    expect(api.GET).toHaveBeenCalledWith('/api/v1/requests/{request_id}', {
      params: { path: { request_id: 'ap1' } },
    })
  })

  it('posts the decision to the decision endpoint', async () => {
    const api = stubApi('pending')
    mount(api)

    fireEvent.click(await screen.findByText('Approve'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/requests/{request_id}/decision',
        {
          params: { path: { request_id: 'ap1' } },
          body: { decision: 'approved' },
        },
      ),
    )
  })

  it('always allow is a checkbox that widens the one Approve', async () => {
    const api = stubApi('pending', ['git status', 'echo'])
    mount(api)

    await screen.findByText('Always allow')
    expect(screen.getByText('git status, echo')).toBeTruthy()
    const always = screen.getByRole('checkbox')
    expect((always as HTMLInputElement).checked).toBe(false)
    fireEvent.click(always)
    fireEvent.click(screen.getByText('Approve'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/requests/{request_id}/decision',
        {
          params: { path: { request_id: 'ap1' } },
          body: { decision: 'approved', scope: 'always' },
        },
      ),
    )
  })

  it('offers no always checkbox when the approval proposes no rules', async () => {
    const api = stubApi('pending', [])
    mount(api)

    expect(await screen.findByText('Approve')).toBeTruthy()
    expect(screen.queryByRole('checkbox')).toBeNull()
  })

  it('collapses a decided approval to one line with the decision and the time', async () => {
    const decidedAt = new Date('2026-01-02T15:04:00Z').getTime()
    const api = stubApi('denied', ['echo'], decidedAt)
    mount(api)

    const settled = await screen.findByTestId('approval-settled')
    expect(settled.textContent).toContain('Denied')
    expect(settled.textContent).toContain(
      new Date(decidedAt).toLocaleTimeString([], {
        hour: '2-digit',
        minute: '2-digit',
      }),
    )
    expect(screen.queryByTestId('approval-card')).toBeNull()
    expect(screen.queryByText('Approve')).toBeNull()
    expect(screen.queryByText('Deny')).toBeNull()
  })

  it('shows a superseded approval as answered by the user message', async () => {
    const api = stubApi('superseded')
    mount(api)

    expect(await screen.findByText('You replied instead')).toBeTruthy()
    expect(screen.queryByText('Approve')).toBeNull()
  })

  it('renders two cards of one approval from the same row state', async () => {
    const api = stubApi('expired')
    mount(api, [cardBlock, cardBlock])

    expect(await screen.findAllByText(/Expired/)).toHaveLength(2)
    expect(screen.queryByText('Approve')).toBeNull()
  })
})

function stubHostApi(proposedRules: string[]) {
  return {
    GET: vi.fn(async () => ({
      data: {
        id: 'ap1',
        agent_id: 'ag1',
        run_id: 'r1',
        kind: 'tool_action',
        payload: {
          tool_name: 'host_shell',
          arguments: { command: 'git status --short' },
          effect_class: 'host',
          host_id: 'h-1',
          host_name: 'Air',
          proposed_rules: proposedRules,
        },
        state: 'pending',
        decided_at: null,
        created_at: 1,
      },
    })),
    POST: vi.fn(async () => ({ data: { state: 'approved' } })),
  }
}

describe('ApprovalCard for a host command', () => {
  const hostBlock = {
    type: 'approval_card',
    request_id: 'ap1',
    title: 'Run a command on your Air',
    body: 'git status --short',
  }

  /** A rule for a program accepts every flag of that program, and some
   *  flags write files or run other programs. The person reads that
   *  where they choose "Always allow". */
  it('states next to Always allow that the rule allows every flag and argument', async () => {
    mount(stubHostApi(['git status']) as unknown as ReturnType<typeof stubApi>, [hostBlock])

    const always = await screen.findByRole('checkbox')
    const scope = document.getElementById(always.getAttribute('aria-describedby') ?? '')
    expect(scope?.textContent).toMatch(/every flag and argument of the command/)
    expect(scope?.textContent).toMatch(/write files or run other programs/)
    expect(always.closest('label')?.nextElementSibling).toBe(scope)
  })

  it('offers only a one-time approval when the command proposes no rule', async () => {
    mount(stubHostApi([]) as unknown as ReturnType<typeof stubApi>, [hostBlock])

    expect(await screen.findByText('Approve')).toBeTruthy()
    expect(screen.queryByRole('checkbox')).toBeNull()
    expect(screen.queryByText(/every flag and argument/)).toBeNull()
  })
})

function stubPluginApi() {
  return {
    GET: vi.fn(async () => ({
      data: {
        id: 'ap1',
        agent_id: 'ag1',
        run_id: 'r1',
        kind: 'tool_action',
        payload: {
          tool_name: 'weather__buy_umbrella',
          effect_class: 'host',
          plugin_id: 'pl-1',
          proposed_rules: ['weather__buy_umbrella'],
        },
        state: 'pending',
        decided_at: null,
        created_at: 1,
      },
    })),
    POST: vi.fn(async () => ({ data: { state: 'approved' } })),
  }
}

describe('ApprovalCard for a plugin tool', () => {
  it('says the tool runs here, and always allows that one tool', async () => {
    const api = stubPluginApi()
    mount(api as unknown as ReturnType<typeof stubApi>, [
      {
        type: 'approval_card',
        request_id: 'ap1',
        title: 'weather__buy_umbrella',
        body: 'weather__buy_umbrella',
      },
    ])

    expect(
      await screen.findByText(/runs on this computer with your privileges/),
    ).toBeTruthy()
    fireEvent.click(screen.getByRole('checkbox'))
    expect(screen.getByText(/Always allow this tool/)).toBeTruthy()
    // The rule is the tool's own name, not a command with flags.
    expect(screen.queryByText(/every flag and argument/)).toBeNull()
    fireEvent.click(screen.getByText('Approve'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/requests/{request_id}/decision',
        {
          params: { path: { request_id: 'ap1' } },
          body: { decision: 'approved', scope: 'always' },
        },
      ),
    )
  })
})

function stubCredentialApi() {
  return {
    GET: vi.fn(async () => ({
      data: {
        id: 'ap1',
        agent_id: 'ag1',
        run_id: 'r1',
        kind: 'credential_action',
        payload: {
          action: 'fill',
          domain: 'example.com',
          username: 'alice@example.com',
          login_url: 'https://example.com/login',
          proposed_rules: ['example.com'],
        },
        state: 'pending',
        decided_at: null,
        created_at: 1,
      },
    })),
    POST: vi.fn(async () => ({ data: { state: 'approved' } })),
  }
}

describe('ApprovalCard for a credential action', () => {
  it('shows the record fields and offers Always allow for the domain', async () => {
    const api = stubCredentialApi()
    mount(api as unknown as ReturnType<typeof stubApi>, [
      {
        type: 'approval_card',
        request_id: 'ap1',
        title: 'Fill a saved login',
        body: 'example.com — alice@example.com — https://example.com/login',
      },
    ])

    // What and where, in one line: who at which site, and the one
    // address the daemon opens.
    expect(
      await screen.findByText('alice@example.com at example.com'),
    ).toBeTruthy()
    expect(screen.getByText('https://example.com/login')).toBeTruthy()

    fireEvent.click(screen.getByRole('checkbox'))
    fireEvent.click(screen.getByText('Approve'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/requests/{request_id}/decision',
        {
          params: { path: { request_id: 'ap1' } },
          body: { decision: 'approved', scope: 'always' },
        },
      ),
    )
  })
})

const SESSION_LABEL = 'Always allow Claude Code sessions in /work/pagis on Air'

function stubSessionStartApi() {
  return {
    GET: vi.fn(async () => ({
      data: {
        id: 'ap1',
        agent_id: 'ag1',
        run_id: 'r1',
        kind: 'tool_action',
        payload: {
          tool_name: 'coding_session_start',
          effect_class: 'host',
          host_id: 'h-1',
          host_name: 'Air',
          proposed_rules: [],
          proposed_session_allow_rule: {
            harness: 'claude',
            directory: '/work/pagis',
          },
          always_label: SESSION_LABEL,
        },
        state: 'pending',
        decided_at: null,
        created_at: 1,
      },
    })),
    POST: vi.fn(async () => ({ data: { state: 'approved' } })),
  }
}

describe('ApprovalCard for a Coding Session start', () => {
  it('names the harness, the directory and the machine beside Always allow', async () => {
    const api = stubSessionStartApi()
    mount(api as unknown as ReturnType<typeof stubApi>, [
      {
        type: 'approval_card',
        request_id: 'ap1',
        title: 'Start Claude Code on your Air',
        body: 'Harness: Claude Code',
      },
    ])

    const always = await screen.findByRole('checkbox')
    expect(always.closest('label')?.textContent).toBe(SESSION_LABEL)
    expect(always.getAttribute('aria-describedby')).toBeNull()
    expect(screen.queryByText(/every flag and argument/)).toBeNull()
    fireEvent.click(always)
    fireEvent.click(screen.getByText('Approve'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/requests/{request_id}/decision',
        {
          params: { path: { request_id: 'ap1' } },
          body: { decision: 'approved', scope: 'always' },
        },
      ),
    )
  })
})
