// Every timeline block draws the same frame: a Frame with one
// radius, a header line that names the act and the place, a body, a
// footer, and a settled line when the block collapses.

import { render, screen } from '@testing-library/react'
import { Mail } from 'lucide-react'
import { describe, expect, it } from 'vitest'

import { Card, CardBody, CardFooter, CardHeader, SettledLine } from './Card'

describe('Card', () => {
  it('composes the Frame primitive', () => {
    const { container } = render(<Card data-testid="card">x</Card>)
    const card = screen.getByTestId('card')
    expect(card.className).toContain('ui-frame')
    expect(card.className).toContain('block-card')
    expect(container.firstElementChild).toBe(card)
  })

  it('names the act and the place on one header line', () => {
    render(
      <Card>
        <CardHeader icon={Mail} act="Send mail" place="to priya@northwind.co">
          <span>Trusted</span>
        </CardHeader>
        <CardBody>Hi Priya</CardBody>
        <CardFooter>Approve</CardFooter>
      </Card>,
    )
    const header = screen.getByText('Send mail').closest('.block-card-header')!
    expect(header.textContent).toContain('to priya@northwind.co')
    expect(header.textContent).toContain('Trusted')
    expect(header.querySelector('svg')).not.toBeNull()
    expect(screen.getByText('Hi Priya').className).toContain('block-card-body')
    expect(screen.getByText('Approve').className).toContain('block-card-footer')
  })

  it('collapses to one line with a dot in the state hue', () => {
    render(
      <Card>
        <SettledLine tone="failed" aside="Denied · 4:58 pm">
          Pay $412.00
        </SettledLine>
      </Card>,
    )
    const line = screen.getByText('Pay $412.00').closest('.block-settled')!
    expect(line.className).toContain('block-settled-failed')
    expect(line.textContent).toContain('Denied · 4:58 pm')
  })
})
