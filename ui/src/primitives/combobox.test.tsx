// The combobox takes typed text, offers the items that match it in a
// list the page draws, and the arrows and Enter choose one.

import { useState } from 'react'
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { Combobox } from './combobox'

const ITEMS = ['openai/gpt-6-luna', 'openai/gpt-6-sol', 'anthropic/claude-sonnet-4-6']

function Harness({ onEnter }: { onEnter?: (value: string) => void }) {
  const [value, setValue] = useState('')
  return (
    <Combobox
      label="Add a candidate"
      value={value}
      onValueChange={setValue}
      items={ITEMS}
      onKeyDown={(event) => {
        if (event.key === 'Enter') onEnter?.(value)
      }}
    />
  )
}

function input() {
  return screen.getByRole('combobox', { name: 'Add a candidate' }) as HTMLInputElement
}

describe('Combobox', () => {
  it('lists only the items that match the typed text', async () => {
    const user = userEvent.setup()
    render(<Harness />)

    await user.type(input(), 'gpt')
    const options = await screen.findAllByRole('option')
    expect(options.map((option) => option.textContent)).toEqual([
      'openai/gpt-6-luna',
      'openai/gpt-6-sol',
    ])
  })

  it('chooses an item on the arrows and Enter without passing the Enter on', async () => {
    const user = userEvent.setup()
    const onEnter = vi.fn()
    render(<Harness onEnter={onEnter} />)

    await user.type(input(), 'sol')
    await screen.findByRole('option', { name: 'openai/gpt-6-sol' })
    await user.keyboard('{ArrowDown}{Enter}')
    expect(input().value).toBe('openai/gpt-6-sol')
    expect(onEnter).not.toHaveBeenCalled()
  })

  it('chooses an item on a click', async () => {
    const user = userEvent.setup()
    render(<Harness />)

    await user.click(input())
    await user.click(await screen.findByRole('option', { name: 'anthropic/claude-sonnet-4-6' }))
    expect(input().value).toBe('anthropic/claude-sonnet-4-6')
  })

  it('keeps typed text that no item matches and passes the Enter on', async () => {
    const user = userEvent.setup()
    const onEnter = vi.fn()
    render(<Harness onEnter={onEnter} />)

    await user.type(input(), 'openrouter/vendor/typed{Enter}')
    expect(screen.queryByRole('option')).toBeNull()
    expect(onEnter).toHaveBeenCalledWith('openrouter/vendor/typed')
  })

})
