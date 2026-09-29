// One rule opens in place: the verdict, one condition per line,
// the count of that rule alone, and Save rule, Cancel, Remove rule.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { RuleEditor } from './RuleEditor'
import type { Catalogue, Rule } from './rules'
import { asClient, catalogue as stubCatalogue, filter, stubApi } from './stub'

const catalogue: Catalogue = {
  signals: [
    ...stubCatalogue.catalogue.signals,
    { id: 'replied', label: 'The owner replied', kind: { kind: 'boolean' } },
    { id: 'newest', label: 'Newest message', kind: { kind: 'date_time' } },
  ],
}

function mount(rule: Rule = filter.rules[1], api = stubApi()) {
  const onSave = vi.fn()
  const onCancel = vi.fn()
  const onRemove = vi.fn()
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  render(
    <QueryClientProvider client={client}>
      <RuleEditor
        api={asClient(api)}
        connectionId="conn-1"
        catalogue={catalogue}
        position={2}
        rule={rule}
        onSave={onSave}
        onCancel={onCancel}
        onRemove={onRemove}
      />
    </QueryClientProvider>,
  )
  return { api, onSave, onCancel, onRemove }
}

function lines() {
  return screen.getAllByRole('group', { name: /^Condition \d+$/ })
}

describe('RuleEditor', () => {
  it('shows the verdict and one line per condition, "when" then "and"', () => {
    mount()
    expect(screen.getByRole('combobox', { name: 'Verdict of rule 2' }).textContent).toContain(
      'Skip',
    )
    expect(screen.getByText('this page when every condition holds')).toBeTruthy()
    const [first, second] = lines()
    expect(first.textContent).toMatch(/^when/)
    expect(
      within(first).getByRole('combobox', { name: 'Signal of condition 1' }).textContent,
    ).toContain('Sender address')
    expect(
      within(first).getByRole('combobox', { name: 'Operator of condition 1' }).textContent,
    ).toContain('is in')
    expect(within(first).getByText('noreply@x.io')).toBeTruthy()
    expect(second.textContent).toMatch(/^and/)
    expect(
      (
        within(second).getByRole('spinbutton', {
          name: 'Value of condition 2',
        }) as HTMLInputElement
      ).value,
    ).toBe('1')
  })

  it('counts the pages this one rule decides, alone', async () => {
    const { api } = mount()
    expect((await screen.findByText('1,000')).parentElement!.textContent).toBe(
      'would skip 1,000 of 3,412 pages',
    )
    const calls = api.POST.mock.calls as unknown as [string, { body: unknown }][]
    const [, request] = calls.find(([path]) => path.endsWith('/filter/preview'))!
    expect(request.body).toEqual({ rules: [filter.rules[1]], default: 'reflect' })
  })

  it('enters a list value as chips and refuses a malformed entry', () => {
    const { onSave } = mount()
    const entry = screen.getByRole('textbox', {
      name: 'Values of condition 1',
    })
    fireEvent.change(entry, { target: { value: 'not an address' } })
    fireEvent.keyDown(entry, { key: 'Enter' })
    expect(screen.queryByText('not an address')).toBeNull()
    fireEvent.change(entry, { target: { value: 'news@letters.example' } })
    fireEvent.keyDown(entry, { key: 'Enter' })
    expect(screen.getByText('news@letters.example')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Remove noreply@x.io' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save rule' }))
    expect(onSave).toHaveBeenCalledWith({
      verdict: 'skip',
      conditions: [
        {
          ...filter.rules[1].conditions[0],
          value: { kind: 'list', values: ['news@letters.example'] },
        },
        filter.rules[1].conditions[1],
      ],
    })
  })

  it('adds a condition as an "and" line and removes one', () => {
    const { onSave } = mount()
    fireEvent.click(screen.getByRole('button', { name: 'Add a condition' }))
    expect(lines()).toHaveLength(3)
    expect(lines()[2].textContent).toMatch(/^and/)
    fireEvent.click(screen.getByRole('button', { name: 'Remove condition 1' }))
    expect(lines()).toHaveLength(2)
    expect(lines()[0].textContent).toMatch(/^when/)
    fireEvent.click(screen.getByRole('button', { name: 'Save rule' }))
    expect(onSave).toHaveBeenCalledWith({
      verdict: 'skip',
      conditions: [
        filter.rules[1].conditions[1],
        {
          signal: 'subject',
          operator: 'contains',
          value: { kind: 'text', value: '' },
        },
      ],
    })
  })

  it('changes the verdict', async () => {
    const user = userEvent.setup()
    const { onSave } = mount()
    await user.click(screen.getByRole('combobox', { name: 'Verdict of rule 2' }))
    await user.click(await screen.findByRole('option', { name: 'Reflect' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save rule' }))
    expect(onSave.mock.calls[0][0].verdict).toBe('reflect')
  })

  it('takes a number, a date and a checkbox', () => {
    const { onSave } = mount({
      verdict: 'reflect',
      conditions: [
        {
          signal: 'messages',
          operator: 'at_least',
          value: { kind: 'number', value: 1 },
        },
        {
          signal: 'newest',
          operator: 'after',
          value: { kind: 'date_time', at: new Date(2026, 0, 2).getTime() },
        },
        {
          signal: 'replied',
          operator: 'is',
          value: { kind: 'boolean', value: true },
        },
      ],
    })
    fireEvent.change(screen.getByRole('spinbutton', { name: 'Value of condition 1' }), {
      target: { value: '4' },
    })
    const date = screen.getByLabelText('Value of condition 2') as HTMLInputElement
    expect(date.value).toBe('2026-01-02')
    fireEvent.change(date, { target: { value: '2026-03-05' } })
    fireEvent.click(screen.getByRole('checkbox', { name: 'The owner replied' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save rule' }))
    expect(onSave.mock.calls[0][0].conditions.map((c: { value: unknown }) => c.value)).toEqual([
      { kind: 'number', value: 4 },
      { kind: 'date_time', at: new Date(2026, 2, 5).getTime() },
      { kind: 'boolean', value: false },
    ])
  })

  it('cancels and removes', () => {
    const { onCancel, onRemove, onSave } = mount()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(onCancel).toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Remove rule' }))
    expect(onRemove).toHaveBeenCalled()
    expect(onSave).not.toHaveBeenCalled()
  })
})
