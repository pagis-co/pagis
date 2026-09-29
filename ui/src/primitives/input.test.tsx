// The field takes the focus on Tab and types. A bare field also drops
// the frame, because the card around it draws one.

import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'

import { Input, Textarea } from './input'

describe('Input', () => {
  it('takes the focus on Tab and takes the keystrokes', async () => {
    const user = userEvent.setup()
    render(<Input aria-label="Channel name" />)

    await user.tab()
    const field = screen.getByLabelText('Channel name')
    expect(document.activeElement).toBe(field)

    await user.keyboard('Sales')
    expect((field as HTMLInputElement).value).toBe('Sales')
  })

  it('skips a disabled field on Tab', async () => {
    const user = userEvent.setup()
    render(
      <>
        <Input aria-label="First" disabled />
        <Input aria-label="Second" />
      </>,
    )
    await user.tab()
    expect(document.activeElement).toBe(screen.getByLabelText('Second'))
  })
})

describe('a bare field', () => {
  it('drops the frame of the field and keeps the field styling', () => {
    render(<Input aria-label="Search" bare />)
    const field = screen.getByLabelText('Search')
    expect(field.className.split(' ')).toEqual(['ui-input', 'ui-input-bare'])
  })

  it('draws the frame when the screen does not ask for a bare field', () => {
    render(<Input aria-label="Channel name" />)
    expect(screen.getByLabelText('Channel name').className).not.toContain(
      'ui-input-bare',
    )
  })

  it('drops the frame of a multi-line field too', () => {
    render(<Textarea aria-label="Message" bare className="composer-input" />)
    const field = screen.getByLabelText('Message')
    expect(field.className.split(' ')).toEqual([
      'ui-input',
      'ui-textarea',
      'ui-input-bare',
      'composer-input',
    ])
  })

  it('writes no bare attribute on the element', () => {
    render(<Input aria-label="Search" bare />)
    expect(screen.getByLabelText('Search').hasAttribute('bare')).toBe(false)
  })
})

describe('Textarea', () => {
  it('takes the focus on Tab and takes a new line', async () => {
    const user = userEvent.setup()
    render(<Textarea aria-label="Personality" />)

    await user.tab()
    await user.keyboard('one{Enter}two')
    expect((screen.getByLabelText('Personality') as HTMLTextAreaElement).value).toBe(
      'one\ntwo',
    )
  })
})
