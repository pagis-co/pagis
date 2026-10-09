// The form and the choice card (ADR-0004): both view a Request
// row. State comes from the row and never from the block — pending is
// interactive, settled is read-only with the submitted values, and
// expired is not submittable. That rule is what makes a submitted form
// legible months later in a persisted timeline.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient, MessageDto } from '../api/client'
import { Blocks } from './BlockView'

const FIELDS = [
  { key: 'who', label: 'Who', kind: 'text', required: true },
  { key: 'seats', label: 'Seats', kind: 'number' },
  {
    key: 'room',
    label: 'Room',
    kind: 'select',
    options: [
      { value: 'a', label: 'Room A' },
      { value: 'b', label: 'Room B' },
    ],
  },
  { key: 'catering', label: 'Catering', kind: 'checkbox' },
]

const OPTIONS = [
  { value: 'tue', label: 'Tuesday' },
  { value: 'wed', label: 'Wednesday' },
]

function stubApi(
  row: {
    kind: string
    payload: unknown
    state: string
    values?: unknown
  },
  post: () => Promise<unknown> = async () => ({ data: { state: 'approved' } }),
) {
  return {
    GET: vi.fn(async () => ({
      data: {
        id: 'rq1',
        agent_id: 'ag1',
        run_id: 'r1',
        values: null,
        decided_at: null,
        created_at: 1,
        ...row,
      },
    })),
    POST: vi.fn(post),
  }
}

const formBlock = {
  type: 'form',
  request_id: 'rq1',
  title: 'Book the room',
  fields: FIELDS,
  submit_label: 'Book',
}

const choiceBlock = {
  type: 'choice_card',
  request_id: 'rq1',
  title: 'Which date?',
  body: 'Both work for me.',
  options: OPTIONS,
}

function mount(api: ReturnType<typeof stubApi>, blocks: unknown) {
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

function pendingForm() {
  return stubApi({
    kind: 'form',
    payload: { title: 'Book the room', fields: FIELDS, submit_label: 'Book' },
    state: 'pending',
  })
}

describe('the form block', () => {
  it('submits its values to the decision endpoint', async () => {
    const user = userEvent.setup()
    const api = pendingForm()
    mount(api, [formBlock])

    fireEvent.change(await screen.findByLabelText(/Who/), {
      target: { value: 'Ada' },
    })
    fireEvent.change(screen.getByLabelText('Seats'), { target: { value: '4' } })
    // The select is a Radix combobox: it is opened, then chosen from.
    await user.click(screen.getByRole('combobox', { name: 'Room' }))
    await user.click(await screen.findByRole('option', { name: 'Room B' }))
    fireEvent.click(screen.getByLabelText('Catering'))
    fireEvent.click(screen.getByRole('button', { name: 'Book' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/requests/{request_id}/decision',
        {
          params: { path: { request_id: 'rq1' } },
          body: {
            decision: 'approved',
            scope: undefined,
            // A number field submits a number, not its input string.
            values: { who: 'Ada', seats: 4, room: 'b', catering: true },
          },
        },
      ),
    )
  })

  it('re-renders read-only with its values once the row settles', async () => {
    // The decision invalidates the row; the card then reads the
    // settled state and the submitted values from it, not from itself.
    let state = 'pending'
    const api = pendingForm()
    api.GET = vi.fn(async () => ({
      data: {
        id: 'rq1',
        agent_id: 'ag1',
        run_id: 'r1',
        kind: 'form',
        payload: { title: 'Book the room', fields: FIELDS },
        state,
        values: state === 'approved' ? { who: 'Ada' } : null,
        decided_at: null,
        created_at: 1,
      },
    }))
    api.POST = vi.fn(async () => {
      state = 'approved'
      return { data: { state: 'approved' } }
    })
    mount(api, [formBlock])

    fireEvent.change(await screen.findByLabelText(/Who/), {
      target: { value: 'Ada' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Book' }))

    expect(await screen.findByText('Answered')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Book' })).toBeNull()
    expect((screen.getByLabelText(/Who/) as HTMLInputElement).value).toBe('Ada')
    expect(screen.getByLabelText(/Who/).hasAttribute('disabled')).toBe(true)
  })

  it('names a missing required field and sends nothing', async () => {
    const api = pendingForm()
    mount(api, [formBlock])

    fireEvent.click(await screen.findByRole('button', { name: 'Book' }))

    expect(await screen.findByRole('alert')).toBeTruthy()
    expect(screen.getByText('This is required.')).toBeTruthy()
    expect(api.POST).not.toHaveBeenCalled()

    // Typing clears the error against that field.
    fireEvent.change(screen.getByLabelText(/Who/), { target: { value: 'Ada' } })
    expect(screen.queryByText('This is required.')).toBeNull()
  })

  it('dismisses with a denial and no values', async () => {
    const api = pendingForm()
    mount(api, [formBlock])

    fireEvent.click(await screen.findByRole('button', { name: 'Dismiss' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/requests/{request_id}/decision',
        {
          params: { path: { request_id: 'rq1' } },
          body: { decision: 'denied', scope: undefined, values: undefined },
        },
      ),
    )
  })

  it('shows the daemon refusal and leaves the form up', async () => {
    const api = pendingForm()
    api.POST = vi.fn(async () => ({
      error: { error: { code: 'validation', message: 'field room is not one of its options' } },
    }))
    mount(api, [formBlock])

    fireEvent.change(await screen.findByLabelText(/Who/), {
      target: { value: 'Ada' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Book' }))

    expect(
      await screen.findByText('field room is not one of its options'),
    ).toBeTruthy()
    // The run stays parked, so the form stays submittable.
    expect(screen.getByRole('button', { name: 'Book' })).toBeTruthy()
  })

  it('renders a settled form read-only with the submitted values', async () => {
    const api = stubApi({
      kind: 'form',
      payload: { title: 'Book the room', fields: FIELDS },
      state: 'approved',
      values: { who: 'Ada', room: 'b', catering: true },
    })
    mount(api, [formBlock])

    expect(await screen.findByText('Answered')).toBeTruthy()
    expect((screen.getByLabelText(/Who/) as HTMLInputElement).value).toBe('Ada')
    // The Radix trigger carries the chosen label, not a value.
    expect(
      screen.getByRole('combobox', { name: 'Room' }).textContent,
    ).toContain('Room B')
    expect((screen.getByLabelText('Catering') as HTMLInputElement).checked).toBe(
      true,
    )
    expect(screen.getByLabelText(/Who/).hasAttribute('disabled')).toBe(true)
    expect(screen.queryByRole('button', { name: 'Book' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Dismiss' })).toBeNull()
  })

  it('renders an expired form disabled with its reason', async () => {
    const api = stubApi({
      kind: 'form',
      payload: { title: 'Book the room', fields: FIELDS },
      state: 'expired',
    })
    mount(api, [formBlock])

    expect(
      await screen.findByText(/the run ended before you answered/),
    ).toBeTruthy()
    expect(screen.getByLabelText(/Who/).hasAttribute('disabled')).toBe(true)
    expect(screen.queryByRole('button', { name: 'Book' })).toBeNull()
  })

  it('reads the field schema from the row, not from the block', async () => {
    // A client can post back a mutated block, so the block's copy is
    // not evidence. The row's own schema is what renders.
    const api = stubApi({
      kind: 'form',
      payload: {
        title: 'Book the room',
        fields: [{ key: 'when', label: 'When', kind: 'date', required: true }],
      },
      state: 'pending',
    })
    mount(api, [formBlock])

    expect(await screen.findByLabelText(/When/)).toBeTruthy()
    expect(screen.queryByLabelText(/Who/)).toBeNull()
  })

  it('is not interactive until the row loads', () => {
    const api = pendingForm()
    mount(api, [formBlock])

    // The block alone renders the question, and nothing more.
    expect(screen.getByText('Book the room')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Book' })).toBeNull()
    expect(screen.getByLabelText(/Who/).hasAttribute('disabled')).toBe(true)
  })

  it('falls back when the block carries no request', () => {
    mount(pendingForm(), [{ type: 'form', title: 'Orphan', fields: [] }])
    expect(screen.getByTestId('unknown-block')).toBeTruthy()
  })
})

function pendingChoice() {
  return stubApi({
    kind: 'choice',
    payload: {
      title: 'Which date?',
      body: 'Both work for me.',
      options: OPTIONS,
    },
    state: 'pending',
  })
}

describe('the choice card block', () => {
  it('submits one tap as the chosen value', async () => {
    const api = pendingChoice()
    mount(api, [choiceBlock])

    expect(screen.getByText('Both work for me.')).toBeTruthy()
    // The options are tappable only once the row says pending.
    await screen.findByRole('button', { name: 'Dismiss' })
    fireEvent.click(screen.getByRole('button', { name: 'Wednesday' }))

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        '/api/v1/requests/{request_id}/decision',
        {
          params: { path: { request_id: 'rq1' } },
          body: {
            decision: 'approved',
            scope: undefined,
            values: { value: 'wed' },
          },
        },
      ),
    )
  })

  it('collapses a settled choice to one line that names the chosen option', async () => {
    const api = stubApi({
      kind: 'choice',
      payload: { title: 'Which date?', options: OPTIONS },
      state: 'approved',
      values: { value: 'tue' },
    })
    mount(api, [choiceBlock])

    const line = await screen.findByTestId('choice-settled')
    expect(line.textContent).toContain('Which date?')
    expect(line.textContent).toContain('Tuesday')
    expect(line.textContent).toContain('Answered')
    // The card collapses: no option and no Dismiss stays on screen.
    expect(line.querySelectorAll('button')).toHaveLength(0)
    expect(screen.queryByText('Wednesday')).toBeNull()
  })

  it('collapses a dismissed choice to one line', async () => {
    const api = stubApi({
      kind: 'choice',
      payload: { title: 'Which date?', options: OPTIONS },
      state: 'denied',
    })
    mount(api, [choiceBlock])

    const line = await screen.findByTestId('choice-settled')
    expect(line.textContent).toContain('Which date?')
    expect(line.textContent).toContain('Dismissed')
    expect(line.querySelectorAll('button')).toHaveLength(0)
  })

  it('shows a superseded choice as answered by the user message', async () => {
    const api = stubApi({
      kind: 'choice',
      payload: { title: 'Which date?', options: OPTIONS },
      state: 'superseded',
    })
    mount(api, [choiceBlock])

    expect(await screen.findByText('You replied instead')).toBeTruthy()
  })

  it('renders two cards of one Request from the same row state', async () => {
    const api = stubApi({
      kind: 'choice',
      payload: { title: 'Which date?', options: OPTIONS },
      state: 'expired',
    })
    mount(api, [choiceBlock, choiceBlock])

    expect(
      await screen.findAllByText(/the run ended before you answered/),
    ).toHaveLength(2)
  })

  it('falls back when the block carries no request', () => {
    mount(pendingChoice(), [
      { type: 'choice_card', title: 'Orphan', options: [] },
    ])
    expect(screen.getByTestId('unknown-block')).toBeTruthy()
  })
})

describe('the choice card block at phone width', () => {
  beforeEach(() => {
    vi.stubGlobal('matchMedia', (query: string) => ({
      matches: query.includes('max-width'),
      media: query,
      addEventListener() {},
      removeEventListener() {},
    }))
  })
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('shows the label and the description of an option, and never its value', async () => {
    mount(
      stubApi({
        kind: 'choice',
        payload: {
          title: 'Which flight do you want?',
          options: [{ value: 'united', label: '07:40 to 11:05', description: 'Direct · Seat 12C' }],
        },
        state: 'pending',
      }),
      [
        {
          ...choiceBlock,
          options: [{ value: 'united', label: '07:40 to 11:05', description: 'Direct · Seat 12C' }],
        },
      ],
    )
    expect(await screen.findByText('07:40 to 11:05')).toBeTruthy()
    expect(screen.getByText('Direct · Seat 12C')).toBeTruthy()
    expect(screen.queryByText('united')).toBeNull()
  })

  it('submits the chosen value, or dismisses the question', async () => {
    const api = pendingChoice()
    mount(api, [choiceBlock])
    fireEvent.click(await screen.findByRole('radio', { name: /Wednesday/ }))
    fireEvent.click(screen.getByRole('button', { name: 'Submit' }))
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/requests/{request_id}/decision', {
        params: { path: { request_id: 'rq1' } },
        body: { decision: 'approved', scope: undefined, values: { value: 'wed' } },
      }),
    )
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }))
    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith('/api/v1/requests/{request_id}/decision', {
        params: { path: { request_id: 'rq1' } },
        body: { decision: 'denied', scope: undefined, values: undefined },
      }),
    )
  })
})
