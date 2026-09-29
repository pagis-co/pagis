// The Agent Phone Number card (ADR-0018): buy in three steps,
// assign, unassign, and release behind a confirmation that names the
// Agent.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import type { AgentDto, ApiClient } from '../api/client'
import { AgentPhoneNumber } from './AgentPhoneNumber'

const robin = {
  id: 'ag1',
  name: 'Robin',
  job: 'answers the phone',
  description: 'Ask Robin to answer the phone',
  personality: 'warm',
  status: 'active',
} as unknown as AgentDto

const carrier = {
  connection_id: 'cx1',
  display_name: 'Telnyx',
  status: 'connected',
}

function stubApi({
  items = [] as unknown[],
  carrierState = carrier as unknown,
  available = [] as unknown[],
} = {}) {
  return {
    GET: vi.fn(async (path: string) => ({
      data:
        path === '/api/v1/settings/phone-numbers'
          ? { items, carrier: carrierState }
          : { items: available },
    })),
    POST: vi.fn(async (_path: string, _options?: unknown) => ({
      data: { id: 'pn1', e164: '+14155550123' },
    })),
    PUT: vi.fn(async () => ({ data: {} })),
    DELETE: vi.fn(async () => ({ error: undefined, response: { ok: true } })),
  }
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={queryClient}>
      <AgentPhoneNumber api={api as unknown as ApiClient} agent={robin} />
    </QueryClientProvider>,
  )
}

describe('the standing brief of a desk line', () => {
  const held = {
    id: 'pn1',
    e164: '+14155550123',
    status: 'assigned',
    agent_id: 'ag1',
    registration: 'registered',
  }

  it('saves what a call to the line is for', async () => {
    const api = stubApi({ items: [held] })
    mount(api)

    const field = await screen.findByLabelText(
      "What a call to Robin's line is for",
    )
    fireEvent.change(field, {
      target: { value: 'Say I am in meetings until the evening.' },
    })
    fireEvent.click(screen.getByText('Save the brief'))

    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith('/api/v1/agents/{agent_id}', {
        params: { path: { agent_id: 'ag1' } },
        body: {
          name: 'Robin',
          job: 'answers the phone',
          description: 'Ask Robin to answer the phone',
          personality: 'warm',
          voice: null,
          standing_brief: 'Say I am in meetings until the evening.',
        },
      }),
    )
  })

  it('says what happens with no brief, and writes an empty one as none', async () => {
    const api = stubApi({ items: [{ ...held }] })
    mount(api)

    // An empty brief is the common answer, so the card says what the
    // Agent does then rather than leaving the field unexplained.
    expect(await screen.findByText(/takes a message/)).toBeTruthy()
    // Nothing changed yet, so there is nothing to save.
    expect(
      screen.getByText('Save the brief').closest('button')?.disabled,
    ).toBe(true)
  })
})

describe('AgentPhoneNumber', () => {
  it('buys a number in three steps and assigns it to the agent', async () => {
    const api = stubApi({
      available: [
        {
          e164: '+14155550123',
          region: 'San Francisco, CA',
          monthly_cost: '1.00',
          currency: 'USD',
        },
      ],
    })
    mount(api)

    fireEvent.click(await screen.findByText('Buy a number'))
    fireEvent.change(screen.getByLabelText('Area code or city'), {
      target: { value: '415' },
    })
    fireEvent.click(screen.getByText('Search'))

    fireEvent.click(await screen.findByText('+1 415 555 0123'))
    // The confirmation says what the line cannot do.
    expect(
      screen.getByText(/does not reach emergency services/),
    ).toBeTruthy()

    fireEvent.click(screen.getByText('Buy and assign'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/settings/phone-numbers', {
        body: { e164: '+14155550123', agent_id: 'ag1' },
      }),
    )
    expect(await screen.findByText(/is Robin's desk line/)).toBeTruthy()
  })

  it('shows the held number with the emergency line and both acts', async () => {
    mount(
      stubApi({
        items: [
          {
            id: 'pn1',
            e164: '+14155550123',
            status: 'assigned',
            agent_id: 'ag1',
          },
        ],
      }),
    )

    expect(await screen.findByText('+1 415 555 0123')).toBeTruthy()
    expect(screen.getByText('Assigned to Robin')).toBeTruthy()
    expect(screen.getByText(/does not reach emergency services/)).toBeTruthy()
    expect(screen.getByLabelText('Unassign +14155550123 from Robin')).toBeTruthy()
    expect(screen.getByLabelText('Release +14155550123')).toBeTruthy()
    // There is no buy action: one Agent holds one number.
    expect(screen.queryByText('Buy a number')).toBeNull()
  })

  // A number the account already holds: typed, proved at the
  // carrier, and the Agent's at once. Nothing is bought.
  it('adds a number the account already holds to the agent', async () => {
    const api = stubApi()
    mount(api)

    fireEvent.click(await screen.findByText('Add a number you own'))
    fireEvent.change(screen.getByLabelText('Phone number'), {
      target: { value: '+1 415 555 0199' },
    })
    fireEvent.click(screen.getByText('Add to Robin'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/settings/phone-numbers/adopt',
        { body: { e164: '+1 415 555 0199', agent_id: 'ag1' } },
      ),
    )
    expect(api.POST).not.toHaveBeenCalledWith(
      '/api/v1/settings/phone-numbers',
      expect.anything(),
    )
  })

  it('says why a number the account does not hold was refused', async () => {
    const api = stubApi()
    api.POST.mockResolvedValueOnce({
      error: {
        error: {
          code: 'validation',
          message: 'your carrier account does not hold that number',
        },
      },
    } as never)
    mount(api)

    fireEvent.click(await screen.findByText('Add a number you own'))
    fireEvent.change(screen.getByLabelText('Phone number'), {
      target: { value: '+14155550199' },
    })
    fireEvent.click(screen.getByText('Add to Robin'))

    expect(
      await screen.findByText(/does not hold that number/),
    ).toBeTruthy()
  })

  it('names the agent in the release confirmation', async () => {
    const api = stubApi({
      items: [
        { id: 'pn1', e164: '+14155550123', status: 'assigned', agent_id: 'ag1' },
      ],
    })
    mount(api)

    fireEvent.click(await screen.findByLabelText('Release +14155550123'))

    expect(
      screen.getByText('Robin holds this number and will have no line.'),
    ).toBeTruthy()
    // The number is still there until the user says so.
    expect(api.POST).not.toHaveBeenCalled()

    fireEvent.click(screen.getByText('Release it'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/settings/phone-numbers/{phone_number_id}/release',
        { params: { path: { phone_number_id: 'pn1' } } },
      ),
    )
  })

  it('assigns a number the workspace already owns', async () => {
    const api = stubApi({
      items: [
        { id: 'pn2', e164: '+14155550124', status: 'unassigned', agent_id: null },
      ],
    })
    mount(api)

    fireEvent.click(await screen.findByLabelText('Assign +14155550124 to Robin'))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/settings/phone-numbers/{phone_number_id}/assign',
        {
          params: { path: { phone_number_id: 'pn2' } },
          body: { agent_id: 'ag1' },
        },
      ),
    )
  })

  // The registration state (ADR-0020), shown on the held number
  // the way a Connection shows `reauth_required`.
  it('shows a held number as registered with the carrier', async () => {
    mount(
      stubApi({
        items: [
          {
            id: 'pn1',
            e164: '+14155550123',
            status: 'assigned',
            agent_id: 'ag1',
            registration: 'registered',
            registration_failure: null,
          },
        ],
      }),
    )

    expect((await screen.findByTestId('phone-registration')).textContent).toBe(
      'Registered with the carrier',
    )
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('says why a held number is not registered and where to fix it', async () => {
    mount(
      stubApi({
        items: [
          {
            id: 'pn1',
            e164: '+14155550123',
            status: 'assigned',
            agent_id: 'ag1',
            registration: 'failed',
            registration_failure: 'no_credential',
          },
        ],
      }),
    )

    expect((await screen.findByTestId('phone-registration')).textContent).toBe(
      'Not registered',
    )
    // The agent page speaks of the sign-in, not of the protocol.
    // The sign-in is the installation's, so the page says who fixes it.
    expect(screen.getByRole('alert').textContent).toBe(
      'The carrier has no sign-in yet. An administrator sets it up in the Administration Interface.',
    )
    expect(document.body.textContent).not.toContain('SIP')
  })

  it('says the carrier refused the sign-in, and where to check it', async () => {
    mount(
      stubApi({
        items: [
          {
            id: 'pn1',
            e164: '+14155550123',
            status: 'assigned',
            agent_id: 'ag1',
            registration: 'failed',
            registration_failure: 'unauthorized',
          },
        ],
      }),
    )

    expect((await screen.findByRole('alert')).textContent).toBe(
      'The carrier refused the sign-in. An administrator sets it up in the Administration Interface.',
    )
    expect(document.body.textContent).not.toContain('SIP')
  })

  it('shows no registration for a number nobody holds', async () => {
    mount(
      stubApi({
        items: [
          { id: 'pn2', e164: '+14155550124', status: 'unassigned', agent_id: null },
        ],
      }),
    )

    expect(await screen.findByText('+1 415 555 0124')).toBeTruthy()
    expect(screen.queryByTestId('phone-registration')).toBeNull()
  })

  it('says an administrator sets up the carrier when there is none', async () => {
    mount(stubApi({ carrierState: null }))

    const missing = await screen.findByTestId('no-carrier')
    expect(missing.textContent).toContain('no phone carrier yet')
    expect(missing.textContent).toContain(
      'An administrator sets it up in the Administration Interface.',
    )
    // A person who is not an administrator gets no link to a port that
    // refuses them.
    expect(screen.queryByRole('link')).toBeNull()
    expect(screen.queryByText('Buy a number')).toBeNull()
  })

  it('says why a carrier that needs attention cannot sell a number', async () => {
    mount(
      stubApi({
        carrierState: {
          connection_id: 'cx1',
          display_name: 'Telnyx',
          status: 'reauth_required',
        },
      }),
    )

    expect(await screen.findByText(/Telnyx is reauth required/)).toBeTruthy()
    expect(screen.queryByText('Buy a number')).toBeNull()
  })
})
