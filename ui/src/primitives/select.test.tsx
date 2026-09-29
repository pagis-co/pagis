// The select opens from the keyboard, the arrows move the highlight,
// Enter chooses and Escape closes without a change.

import { useState } from 'react'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { Select } from './select'

const ITEMS = [
  { value: 'sage', label: 'Sage' },
  { value: 'clown', label: 'Clown' },
]

function Harness({ onChange }: { onChange?: (value: string) => void }) {
  const [value, setValue] = useState('sage')
  return (
    <Select
      label="Responsible agent"
      value={value}
      items={ITEMS}
      onValueChange={(next) => {
        setValue(next)
        onChange?.(next)
      }}
    />
  )
}

describe('Select', () => {
  it('is named by its label and shows the chosen item', () => {
    render(<Harness />)
    expect(screen.getByRole('combobox', { name: 'Responsible agent' }).textContent).toContain(
      'Sage',
    )
  })

  it('opens on Enter, moves on the arrows and chooses on Enter', async () => {
    const user = userEvent.setup()
    const onChange = vi.fn()
    render(<Harness onChange={onChange} />)

    await user.tab()
    expect(document.activeElement).toBe(
      screen.getByRole('combobox', { name: 'Responsible agent' }),
    )

    await user.keyboard('{Enter}')
    expect(await screen.findByRole('option', { name: 'Clown' })).toBeTruthy()

    await user.keyboard('{ArrowDown}{Enter}')
    expect(onChange).toHaveBeenCalledWith('clown')
  })

  it('closes on Escape and keeps the value', async () => {
    const user = userEvent.setup()
    const onChange = vi.fn()
    render(<Harness onChange={onChange} />)

    await user.tab()
    await user.keyboard('{Enter}')
    expect(await screen.findByRole('option', { name: 'Clown' })).toBeTruthy()

    await user.keyboard('{Escape}')
    expect(screen.queryByRole('option', { name: 'Clown' })).toBeNull()
    expect(onChange).not.toHaveBeenCalled()
  })
})
