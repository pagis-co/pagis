// A rule reads as one sentence: the verdict, then "when" and the
// conditions joined by "and". The values are the strong words.

import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import type { Catalogue } from './rules'
import { RuleSentence } from './RuleSentence'

const catalogue: Catalogue = {
  signals: [
    { id: 'labels', label: 'Labels', kind: { kind: 'tag_set', options: ['IMPORTANT'] } },
    { id: 'sender', label: 'Sender address', kind: { kind: 'list', format: 'email' } },
    { id: 'messages', label: 'Messages', kind: { kind: 'number' } },
    { id: 'replied', label: 'The owner replied', kind: { kind: 'boolean' } },
    { id: 'newest', label: 'Newest message', kind: { kind: 'date_time' } },
  ],
}

describe('RuleSentence', () => {
  it('joins the conditions with "and" and makes the values strong', () => {
    const { container } = render(
      <RuleSentence
        catalogue={catalogue}
        conditions={[
          { signal: 'sender', operator: 'in', value: { kind: 'list', values: ['noreply@x.io', '@news.example'] } },
          { signal: 'messages', operator: 'at_most', value: { kind: 'number', value: 1 } },
        ]}
      />,
    )
    expect(container.textContent).toBe(
      'when Sender address is in noreply@x.io, @news.example and Messages is at most 1',
    )
    expect([...container.querySelectorAll('strong')].map((node) => node.textContent)).toEqual([
      'noreply@x.io',
      '@news.example',
      '1',
    ])
  })

  it('reads a tag, a boolean, a duration and a moment', () => {
    const { container } = render(
      <RuleSentence
        catalogue={catalogue}
        conditions={[
          { signal: 'labels', operator: 'has', value: { kind: 'choice', value: 'IMPORTANT' } },
          { signal: 'replied', operator: 'is', value: { kind: 'boolean', value: true } },
          { signal: 'newest', operator: 'within_last', value: { kind: 'duration', amount: 30, unit: 'days' } },
          { signal: 'newest', operator: 'after', value: { kind: 'date_time', at: Date.UTC(2025, 7, 9, 12) } },
        ]}
      />,
    )
    expect(container.textContent).toContain('when Labels has IMPORTANT and The owner replied and Newest message is within the last 30 days and Newest message is after ')
    expect(screen.getByText(/2025/)).toBeTruthy()
  })

  it('says a rule with no condition always holds', () => {
    const { container } = render(<RuleSentence catalogue={catalogue} conditions={[]} />)
    expect(container.textContent).toBe('always')
  })

  it('names an unknown signal by its id', () => {
    const { container } = render(
      <RuleSentence
        catalogue={catalogue}
        conditions={[{ signal: 'gone', operator: 'is', value: { kind: 'text', value: 'x' } }]}
      />,
    )
    expect(container.textContent).toBe('when gone is x')
  })
})
