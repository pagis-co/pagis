// The Agent Mailbox card (ADR-0019):
// the five states, the Outgoing Cap with today's count, the Standing
// Mail Rule with pause, resume and edit, the reset that pastes a
// password on a manual host, the delete behind the typed address, and
// the provision for an Agent that holds none.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { AgentDto, ApiClient } from '../api/client'
import { AgentMailbox } from './AgentMailbox'

const ada = {
  id: 'ag1',
  name: 'Ada',
  job: 'writes letters',
  personality: 'precise',
  status: 'active',
} as unknown as AgentDto

const migadu = {
  id: 'cx1',
  provider: 'migadu',
  alias: 'mail',
  display_name: 'Agent mail',
  status: 'connected',
  authorized_capabilities: [],
  created_at: 1,
  mail: {
    domain: 'example.com',
    imap_host: 'imap.migadu.com',
    imap_port: 993,
    smtp_host: 'smtp.migadu.com',
    smtp_port: 465,
    idle: true,
    outgoing_cap: true,
    delete_mailbox: true,
    reset_password: true,
  },
}

const manual = {
  ...migadu,
  id: 'cx2',
  provider: 'manual',
  display_name: 'Mail host',
  mail: {
    ...migadu.mail,
    imap_host: 'imap.example.com',
    smtp_host: 'smtp.example.com',
    idle: false,
    outgoing_cap: false,
    delete_mailbox: false,
    reset_password: false,
  },
}

const offer = {
  connection_id: 'cx1',
  display_name: 'Agent mail',
  domain: 'example.com',
  suggested_local_part: 'ada',
  default_outgoing_cap: 20,
  mints_password: true,
  deletes_mailbox: true,
}

/** The Standing Mail Rule the daemon writes with the mailbox
 *  (ADR-0019): one Event Subscription on the Agent, for its own
 *  mailbox. */
const standingRule = {
  id: 'sub1',
  agent_id: 'ag1',
  connection_id: 'cx1',
  channel_id: 'ch1',
  workspace_id: 'ws1',
  event_kind: 'mail.message_received',
  filter: { mailbox: 'own' },
  name: 'Inbound mail',
  instruction: 'You received mail. Read it with `mail__get_message`.',
  state: 'active',
  creator: 'user',
  revision: 1,
  source_version: '1',
  created_at: 1,
  updated_at: 1,
  blocked_reason: null,
}

const activeMailbox = {
  id: 'mb1',
  agent_id: 'ag1',
  address: 'ada@example.com',
  state: 'active',
  reason: null,
  connection_id: 'cx1',
  outgoing_cap: 20,
  sends_today: 3,
  created_at: 1,
  deleted_at: null,
}

function stubApi({
  mailbox = null as unknown,
  offers = [] as unknown[],
  connections = [migadu] as unknown[],
  nameCheck = { address: 'ada@example.com', available: true, reason: null } as {
    address: string
    available: boolean
    reason: string | null
  },
  subscriptions = [] as unknown[],
  post = undefined as ((path: string) => Promise<unknown>) | undefined,
  del = undefined as (() => Promise<unknown>) | undefined,
} = {}) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === '/api/v1/agents/{agent_id}/mailbox') {
        return { data: { mailbox, offers } }
      }
      if (path === '/api/v1/settings/connections') {
        return { data: { items: connections } }
      }
      if (path === '/api/v1/settings/mailbox-offers/name') {
        return { data: nameCheck }
      }
      if (path === '/api/v1/event-subscriptions') {
        return { data: { items: subscriptions, next_cursor: null } }
      }
      return { data: { items: [] } }
    }),
    POST: vi.fn(
      post ?? (async () => ({ data: { ...activeMailbox, state: 'provisioning' } })),
    ),
    PUT: vi.fn(async () => ({ data: {} })),
    DELETE: vi.fn(
      del ??
        (async () => ({
          data: { mailbox: { ...activeMailbox, state: 'deleted' }, notice: null },
        })),
    ),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <AgentMailbox api={api as unknown as ApiClient} agent={ada} />
    </QueryClientProvider>,
  )
}

describe('AgentMailbox', () => {
  it('shows the address, the state and the cap with the day count', async () => {
    mount(stubApi({ mailbox: activeMailbox }))

    expect(await screen.findByText('ada@example.com')).toBeTruthy()
    expect(screen.getByTestId('mailbox-state').textContent).toBe('Active')
    expect(screen.getByTestId('mailbox-cap').textContent).toContain(
      'Outgoing cap 20 a day',
    )
    expect(screen.getByTestId('mailbox-cap').textContent).toContain(
      '3 sent today',
    )
  })

  it.each([
    ['provisioning', 'Being made at the host'],
    ['active', 'Active'],
    ['unavailable', 'Unavailable'],
    ['dormant', 'Sprite archived'],
    ['deleted', 'Deleted'],
  ])('names the %s state', async (state, label) => {
    mount(stubApi({ mailbox: { ...activeMailbox, state } }))

    expect((await screen.findByTestId('mailbox-state')).textContent).toBe(label)
  })

  it('says why an unavailable mailbox is unavailable', async () => {
    mount(
      stubApi({
        mailbox: {
          ...activeMailbox,
          state: 'unavailable',
          reason: 'the mail host refused the login',
        },
      }),
    )

    expect(
      await screen.findByText('the mail host refused the login'),
    ).toBeTruthy()
  })

  it('offers a password reset only while the mailbox is unavailable', async () => {
    mount(stubApi({ mailbox: activeMailbox }))

    expect(await screen.findByText('ada@example.com')).toBeTruthy()
    expect(screen.queryByText('Reset password')).toBeNull()
  })

  it('resets the password without one where the host mints it', async () => {
    const api = stubApi({ mailbox: { ...activeMailbox, state: 'unavailable' } })
    mount(api)

    fireEvent.click(await screen.findByText('Reset password'))
    expect(screen.queryByLabelText('New mailbox password')).toBeNull()
    fireEvent.click(screen.getByLabelText('Reset the mailbox password'))

    await waitFor(() => {
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/mailbox/reset-password',
        expect.objectContaining({ body: { password: undefined } }),
      )
    })
  })

  it('pastes a new password where the manual host mints none', async () => {
    const api = stubApi({
      mailbox: { ...activeMailbox, state: 'unavailable', connection_id: 'cx2' },
      connections: [manual],
    })
    mount(api)

    fireEvent.click(await screen.findByText('Reset password'))
    const field = screen.getByLabelText('New mailbox password')
    // Nothing is sent until the user pastes one.
    expect(
      screen.getByLabelText('Reset the mailbox password').hasAttribute('disabled'),
    ).toBe(true)
    fireEvent.change(field, { target: { value: 'pasted-secret' } })
    fireEvent.click(screen.getByLabelText('Reset the mailbox password'))

    await waitFor(() => {
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/mailbox/reset-password',
        expect.objectContaining({ body: { password: 'pasted-secret' } }),
      )
    })
  })

  it('deletes only once the address is typed back', async () => {
    const api = stubApi({ mailbox: activeMailbox })
    mount(api)

    fireEvent.click(await screen.findByLabelText('Delete the mailbox of Ada'))
    const confirm = screen.getByLabelText('Delete ada@example.com')
    expect(confirm.hasAttribute('disabled')).toBe(true)

    fireEvent.change(screen.getByLabelText('Type the address to confirm'), {
      target: { value: 'ada@example.co' },
    })
    expect(confirm.hasAttribute('disabled')).toBe(true)

    fireEvent.change(screen.getByLabelText('Type the address to confirm'), {
      target: { value: 'ada@example.com' },
    })
    expect(confirm.hasAttribute('disabled')).toBe(false)
    fireEvent.click(confirm)

    await waitFor(() => {
      expect(api.DELETE).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/mailbox',
        expect.objectContaining({ body: { confirm_address: 'ada@example.com' } }),
      )
    })
  })

  it('warns that a manual host keeps the inbox after a delete', async () => {
    mount(
      stubApi({
        mailbox: { ...activeMailbox, connection_id: 'cx2' },
        connections: [manual],
      }),
    )

    fireEvent.click(await screen.findByLabelText('Delete the mailbox of Ada'))
    expect(
      screen.getByText(/delete the inbox at the host yourself/),
    ).toBeTruthy()
  })

  it('renders the daemon refusal of a delete', async () => {
    const api = stubApi({
      mailbox: activeMailbox,
      del: async () => ({
        error: { error: { message: 'type the address to delete this mailbox' } },
      }),
    })
    mount(api)

    fireEvent.click(await screen.findByLabelText('Delete the mailbox of Ada'))
    fireEvent.change(screen.getByLabelText('Type the address to confirm'), {
      target: { value: 'ada@example.com' },
    })
    fireEvent.click(screen.getByLabelText('Delete ada@example.com'))

    expect(
      await screen.findByText('type the address to delete this mailbox'),
    ).toBeTruthy()
  })

  it('points at Connections when no mailbox provider is connected', async () => {
    mount(stubApi({ mailbox: null, offers: [] }))

    expect(await screen.findByTestId('no-mailbox-provider')).toBeTruthy()
    expect(screen.queryByText('Provision a mailbox')).toBeNull()
  })

  it('provisions a mailbox for an Agent that holds none', async () => {
    const api = stubApi({ mailbox: null, offers: [offer] })
    mount(api)

    fireEvent.click(await screen.findByText('Provision a mailbox'))
    expect(screen.getByText('@example.com')).toBeTruthy()
    expect(
      (screen.getByLabelText('Mailbox name') as HTMLInputElement).value,
    ).toBe('ada')
    expect(
      (await screen.findByTestId('mailbox-name-check')).textContent,
    ).toContain('ada@example.com is free.')

    fireEvent.change(screen.getByLabelText('Outgoing cap'), {
      target: { value: '5' },
    })
    fireEvent.click(screen.getByLabelText('Make the mailbox for Ada'))

    await waitFor(() => {
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/mailbox',
        expect.objectContaining({
          body: {
            connection_id: 'cx1',
            local_part: 'ada',
            outgoing_cap: 5,
            password: undefined,
          },
        }),
      )
    })
  })

  it('says a name the Address Ledger already holds is taken', async () => {
    mount(
      stubApi({
        mailbox: null,
        offers: [offer],
        nameCheck: {
          address: 'ada@example.com',
          available: false,
          reason: 'ada@example.com is in the address ledger.',
        },
      }),
    )

    fireEvent.click(await screen.findByText('Provision a mailbox'))
    expect(
      (await screen.findByTestId('mailbox-name-check')).textContent,
    ).toContain('is in the address ledger')
  })

  it('takes the password of an inbox made at a manual host', async () => {
    const api = stubApi({
      mailbox: null,
      offers: [
        {
          ...offer,
          connection_id: 'cx2',
          display_name: 'Mail host',
          mints_password: false,
          deletes_mailbox: false,
        },
      ],
      connections: [manual],
    })
    mount(api)

    fireEvent.click(await screen.findByText('Provision a mailbox'))
    const make = screen.getByLabelText('Make the mailbox for Ada')
    expect(make.hasAttribute('disabled')).toBe(true)

    fireEvent.change(screen.getByLabelText('Mailbox password'), {
      target: { value: 'inbox-secret' },
    })
    fireEvent.click(make)

    await waitFor(() => {
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/agents/{agent_id}/mailbox',
        expect.objectContaining({
          body: expect.objectContaining({ password: 'inbox-secret' }),
        }),
      )
    })
  })

  it('renders the host refusal of a provision', async () => {
    const api = stubApi({
      mailbox: null,
      offers: [offer],
      post: async () => ({
        error: {
          error: { message: 'the mail host already holds that address.' },
        },
      }),
    })
    mount(api)

    fireEvent.click(await screen.findByText('Provision a mailbox'))
    fireEvent.click(screen.getByLabelText('Make the mailbox for Ada'))

    expect(
      await screen.findByText('the mail host already holds that address.'),
    ).toBeTruthy()
  })
})

// The Standing Mail Rule on the card (ADR-0019). The
// rule is an Event Subscription, so the card reads and writes it
// through the Event Subscription routes, as the Automations page does.
describe('AgentMailbox standing mail rule', () => {
  it('shows the rule instruction and its active state', async () => {
    mount(stubApi({ mailbox: activeMailbox, subscriptions: [standingRule] }))

    expect(await screen.findByTestId('standing-mail-rule')).toBeTruthy()
    expect(screen.getByTestId('standing-rule-state').textContent).toBe('Active')
    expect(screen.getByTestId('standing-rule-instruction').textContent).toBe(
      'You received mail. Read it with `mail__get_message`.',
    )
  })

  it('reads only the rule of this Agent for its own mailbox', async () => {
    mount(
      stubApi({
        mailbox: activeMailbox,
        subscriptions: [
          // Another Agent's rule.
          { ...standingRule, id: 'sub2', agent_id: 'ag2' },
          // This Agent's rule for a user mail account, not its own.
          {
            ...standingRule,
            id: 'sub3',
            filter: { mailbox: 'gmail' },
            instruction: 'Watch the user inbox.',
          },
          // An archived rule of an earlier mailbox.
          { ...standingRule, id: 'sub4', state: 'archived' },
          standingRule,
        ],
      }),
    )

    expect(
      (await screen.findByTestId('standing-rule-instruction')).textContent,
    ).toBe('You received mail. Read it with `mail__get_message`.')
  })

  it('says the mailbox has no rule where none stands', async () => {
    mount(stubApi({ mailbox: activeMailbox, subscriptions: [] }))

    expect(await screen.findByTestId('standing-rule-missing')).toBeTruthy()
    expect(screen.queryByTestId('standing-mail-rule')).toBeNull()
  })

  it('pauses the rule', async () => {
    const api = stubApi({ mailbox: activeMailbox, subscriptions: [standingRule] })
    mount(api)

    fireEvent.click(await screen.findByLabelText('Pause the standing mail rule'))

    await waitFor(() => {
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/event-subscriptions/{subscription_id}',
        expect.objectContaining({
          params: { path: { subscription_id: 'sub1' } },
          body: { action: 'pause' },
        }),
      )
    })
  })

  it('resumes a paused rule and says no mail wakes the Agent', async () => {
    const api = stubApi({
      mailbox: activeMailbox,
      subscriptions: [{ ...standingRule, state: 'paused' }],
    })
    mount(api)

    expect((await screen.findByTestId('standing-rule-state')).textContent).toBe(
      'Paused',
    )
    expect(screen.getByTestId('standing-rule-paused')).toBeTruthy()
    fireEvent.click(screen.getByLabelText('Resume the standing mail rule'))

    await waitFor(() => {
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/event-subscriptions/{subscription_id}',
        expect.objectContaining({ body: { action: 'resume' } }),
      )
    })
  })

  it('says why a blocked rule is blocked', async () => {
    mount(
      stubApi({
        mailbox: activeMailbox,
        subscriptions: [
          {
            ...standingRule,
            state: 'blocked',
            blocked_reason: 'the mail connection must be connected again',
          },
        ],
      }),
    )

    expect((await screen.findByTestId('standing-rule-state')).textContent).toBe(
      'Blocked',
    )
    expect(
      screen.getByText('the mail connection must be connected again'),
    ).toBeTruthy()
  })

  it('edits the instruction', async () => {
    const api = stubApi({ mailbox: activeMailbox, subscriptions: [standingRule] })
    mount(api)

    fireEvent.click(await screen.findByLabelText('Edit the standing mail rule'))
    fireEvent.change(screen.getByLabelText('Standing mail rule instruction'), {
      target: { value: 'Read the mail and tell me what it says.' },
    })
    fireEvent.click(screen.getByText('Save'))

    await waitFor(() => {
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/event-subscriptions/{subscription_id}',
        expect.objectContaining({
          body: {
            action: 'edit',
            instruction: 'Read the mail and tell me what it says.',
          },
        }),
      )
    })
  })

  it('sends nothing where the edit leaves the instruction empty', async () => {
    const api = stubApi({ mailbox: activeMailbox, subscriptions: [standingRule] })
    mount(api)

    fireEvent.click(await screen.findByLabelText('Edit the standing mail rule'))
    fireEvent.change(screen.getByLabelText('Standing mail rule instruction'), {
      target: { value: '   ' },
    })
    expect(screen.getByText('Save').hasAttribute('disabled')).toBe(true)
  })

  it('renders the daemon refusal of a pause', async () => {
    const api = stubApi({
      mailbox: activeMailbox,
      subscriptions: [standingRule],
      post: async () => ({
        error: { error: { message: 'that rule is archived' } },
      }),
    })
    mount(api)

    fireEvent.click(await screen.findByLabelText('Pause the standing mail rule'))

    expect(await screen.findByText('that rule is archived')).toBeTruthy()
  })

  it('shows no rule where the Agent holds no mailbox', async () => {
    mount(stubApi({ mailbox: null, offers: [], subscriptions: [standingRule] }))

    expect(await screen.findByTestId('no-mailbox-provider')).toBeTruthy()
    expect(screen.queryByTestId('standing-mail-rule')).toBeNull()
    expect(screen.queryByTestId('standing-rule-missing')).toBeNull()
  })
})
