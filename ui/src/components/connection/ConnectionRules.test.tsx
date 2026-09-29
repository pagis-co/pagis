// The What reflects section: the rules as sentences, the
// default verdict as the last line, and the live counts. Edit opens one
// rule in place.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { ConnectionRules } from './ConnectionRules'
import { asClient, catalogue, filter, status, stubApi } from './stub'

function editors() {
  return screen.queryAllByRole('combobox', { name: /^Verdict of rule/ })
}

function mount(onChange = vi.fn(), api = stubApi()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  render(
    <QueryClientProvider client={client}>
      <ConnectionRules
        api={asClient(api)}
        connectionId="conn-1"
        catalogue={catalogue.catalogue}
        filter={filter}
        backfill={{ reflected: status.backfill_reflected, pending: status.backfill_pending }}
        onChange={onChange}
      />
    </QueryClientProvider>,
  )
  return { onChange, api }
}

describe('ConnectionRules', () => {
  it('reads every rule as a numbered sentence with its verdict', () => {
    mount()
    const first = screen.getByText(/when Labels has/).closest('.ui-row')!
    expect(first.textContent).toContain('1.')
    expect(first.textContent).toContain('Reflect')
    expect(first.textContent).toContain('when Labels has IMPORTANT')
    const second = screen.getByText(/Sender address/).closest('.ui-row')!
    expect(second.textContent).toContain('2.')
    expect(second.textContent).toContain('Skip')
    expect(second.textContent).toContain(
      'when Sender address is in noreply@x.io and Messages is at most 1',
    )
  })

  it('ends with the default verdict as the otherwise line', () => {
    const { onChange } = mount()
    const last = screen.getByText('otherwise').closest('.ui-row')!
    expect(last.textContent).toContain('Skip')
    fireEvent.click(screen.getByRole('button', { name: 'Change the default verdict' }))
    expect(onChange).toHaveBeenCalledWith({ ...filter, default: 'reflect' })
  })

  it('opens a new rule in place and adds it on Save rule', () => {
    const { onChange } = mount()
    fireEvent.click(screen.getByRole('button', { name: 'Add a rule' }))
    expect(onChange).not.toHaveBeenCalled()
    expect(screen.getByRole('combobox', { name: 'Verdict of rule 3' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Save rule' }))
    expect(onChange).toHaveBeenCalledWith({
      ...filter,
      rules: [
        ...filter.rules,
        {
          verdict: 'reflect',
          conditions: [
            {
              signal: 'subject',
              operator: 'contains',
              value: { kind: 'text', value: '' },
            },
          ],
        },
      ],
    })
    expect(editors()).toHaveLength(0)
  })

  it('drops a new rule on Cancel', () => {
    const { onChange } = mount()
    fireEvent.click(screen.getByRole('button', { name: 'Add a rule' }))
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(editors()).toHaveLength(0)
    expect(onChange).not.toHaveBeenCalled()
  })

  it('removes a rule', () => {
    const { onChange } = mount()
    fireEvent.click(screen.getByRole('button', { name: 'Remove rule 2' }))
    expect(onChange).toHaveBeenCalledWith({
      ...filter,
      rules: [filter.rules[0]],
    })
  })

  it('moves a rule from the keyboard on its handle', () => {
    const { onChange } = mount()
    fireEvent.keyDown(screen.getByRole('button', { name: 'Move rule 2' }), {
      key: 'ArrowUp',
    })
    expect(onChange).toHaveBeenCalledWith({
      ...filter,
      rules: [filter.rules[1], filter.rules[0]],
    })
  })

  it('moves a rule when it is dragged onto another', () => {
    const { onChange } = mount()
    const rows = screen
      .getAllByRole('button', { name: /^Move rule/ })
      .map((handle) => handle.closest('.ui-row')!)
    fireEvent.dragStart(rows[0])
    fireEvent.dragOver(rows[1])
    fireEvent.drop(rows[1])
    expect(onChange).toHaveBeenCalledWith({
      ...filter,
      rules: [filter.rules[1], filter.rules[0]],
    })
  })

  it('opens one rule in place behind Edit, one rule at a time', () => {
    mount()
    expect(editors()).toHaveLength(0)
    fireEvent.click(screen.getByRole('button', { name: 'Edit rule 1' }))
    expect(editors().map((editor) => editor.getAttribute('aria-label'))).toEqual([
      'Verdict of rule 1',
    ])
    expect(screen.queryByText(/when Labels has/)).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Edit rule 2' }))
    expect(editors().map((editor) => editor.getAttribute('aria-label'))).toEqual([
      'Verdict of rule 2',
    ])
    expect(screen.getByText(/when Labels has/)).toBeTruthy()
  })

  it('saves the edited rule in its place', () => {
    const { onChange } = mount()
    fireEvent.click(screen.getByRole('button', { name: 'Edit rule 2' }))
    fireEvent.click(screen.getByRole('button', { name: 'Remove condition 2' }))
    fireEvent.click(screen.getByRole('button', { name: 'Save rule' }))
    expect(onChange).toHaveBeenCalledWith({
      ...filter,
      rules: [filter.rules[0], { ...filter.rules[1], conditions: [filter.rules[1].conditions[0]] }],
    })
    expect(editors()).toHaveLength(0)
  })

  it('closes the editor on Cancel and keeps the rule', () => {
    const { onChange } = mount()
    fireEvent.click(screen.getByRole('button', { name: 'Edit rule 1' }))
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(editors()).toHaveLength(0)
    expect(screen.getByText(/when Labels has/)).toBeTruthy()
    expect(onChange).not.toHaveBeenCalled()
  })

  it('removes the open rule', () => {
    const { onChange } = mount()
    fireEvent.click(screen.getByRole('button', { name: 'Edit rule 1' }))
    fireEvent.click(screen.getByRole('button', { name: 'Remove rule' }))
    expect(onChange).toHaveBeenCalledWith({
      ...filter,
      rules: [filter.rules[1]],
    })
    expect(editors()).toHaveLength(0)
  })

  it('says how many stored pages the rules reflect', async () => {
    mount()
    expect(
      await screen.findByText(
        /Of 3,412 pages, 1,180 reflect under these rules · Backfill 940 reflected · 240 waiting/,
      ),
    ).toBeTruthy()
  })
})
