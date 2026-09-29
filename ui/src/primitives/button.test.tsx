// The button answers the keyboard: Tab reaches it, Enter and Space
// press it, and a disabled button does neither.

import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'

import { Button } from './button'

describe('Button', () => {
  it('takes the focus on Tab and presses on Enter and on Space', async () => {
    const user = userEvent.setup()
    const press = vi.fn()
    render(<Button onClick={press}>Save</Button>)

    await user.tab()
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Save' }))

    await user.keyboard('{Enter}')
    await user.keyboard(' ')
    expect(press).toHaveBeenCalledTimes(2)
  })

  it('is not reachable and does not press when it is disabled', async () => {
    const user = userEvent.setup()
    const press = vi.fn()
    render(<Button disabled onClick={press}>Save</Button>)

    await user.tab()
    expect(document.activeElement).toBe(document.body)
    expect(press).not.toHaveBeenCalled()
  })

  it('does not submit a form unless it asks to', () => {
    render(
      <>
        <Button>Plain</Button>
        <Button type="submit">Submit</Button>
      </>,
    )
    expect(screen.getByRole('button', { name: 'Plain' }).getAttribute('type')).toBe('button')
    expect(screen.getByRole('button', { name: 'Submit' }).getAttribute('type')).toBe('submit')
  })

  it('carries the variant and the size in its class', () => {
    render(
      <Button variant="danger" size="lg">
        Delete
      </Button>,
    )
    const button = screen.getByRole('button', { name: 'Delete' })
    expect(button.className).toContain('ui-button-danger')
    expect(button.className).toContain('ui-button-lg')
  })

  it('names the shape that fills a row of a frame', () => {
    render(
      <>
        <Button shape="row">Open the mail</Button>
        <Button shape="pill">Filter</Button>
        <Button>Plain</Button>
      </>,
    )
    expect(screen.getByRole('button', { name: 'Open the mail' }).className).toContain(
      'ui-button-row',
    )
    expect(screen.getByRole('button', { name: 'Filter' }).className).toContain(
      'ui-button-pill',
    )
    expect(screen.getByRole('button', { name: 'Plain' }).className).not.toContain(
      'ui-button-default',
    )
  })

  it('is the 32 px outline button by default', () => {
    render(<Button>Replace</Button>)
    const button = screen.getByRole('button', { name: 'Replace' })
    expect(button.className).toContain('ui-button-outline')
    expect(button.className).toContain('ui-button-md')
  })

  it('has the four looks and the 26 px inline ghost', () => {
    render(
      <>
        <Button variant="ghost" size="sm">
          Undo
        </Button>
        <Button variant="primary">Add a sign-in</Button>
        <Button variant="danger">Delete</Button>
      </>,
    )
    expect(screen.getByRole('button', { name: 'Undo' }).className).toContain(
      'ui-button-ghost ui-button-sm',
    )
    expect(screen.getByRole('button', { name: 'Add a sign-in' }).className).toContain(
      'ui-button-primary',
    )
    expect(screen.getByRole('button', { name: 'Delete' }).className).toContain(
      'ui-button-danger',
    )
  })
})
