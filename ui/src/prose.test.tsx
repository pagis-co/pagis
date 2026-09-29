// The prose renderer. One component draws every Markdown
// surface, and it speaks GitHub Flavored Markdown: agents write pipe
// tables, task lists, strikethrough and bare links, and CommonMark
// alone renders none of them. A wide table scrolls in its own box. An
// image shows as a link and never loads.

import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'

import { Prose } from './prose'

describe('Prose', () => {
  it('renders a pipe table as a grid in its own scroll box', () => {
    render(
      <Prose>
        {[
          '| Plan | Provider | Price |',
          '|---|---|---:|',
          '| Fibre 900 | Cascade Link | $89 |',
          '| Fibre 500 | Northwind Broadband | $64 |',
        ].join('\n')}
      </Prose>,
    )

    const table = screen.getByRole('table')
    expect(table.parentElement?.className).toBe('prose-table')
    expect(screen.getAllByRole('row')).toHaveLength(3)
    expect(screen.getByRole('cell', { name: 'Northwind Broadband' })).toBeDefined()
  })

  it('keeps the column alignment the agent wrote', () => {
    render(<Prose>{'| Price |\n|---:|\n| $89 |\n'}</Prose>)

    const cell = screen.getByRole('cell', { name: '$89' })
    expect(cell.style.textAlign).toBe('right')
  })

  it('renders strikethrough, a task list and a bare link', () => {
    render(
      <Prose>
        {'~~gone~~ at https://example.com/rates\n\n- [x] booked\n- [ ] paid\n'}
      </Prose>,
    )

    expect(screen.getByText('gone').tagName).toBe('DEL')
    expect(
      screen.getByRole('link', { name: 'https://example.com/rates' }).getAttribute('href'),
    ).toBe('https://example.com/rates')
    const boxes = screen.getAllByRole('checkbox') as HTMLInputElement[]
    expect(boxes.map((box) => box.checked)).toEqual([true, false])
  })

  it('renders no raw HTML an agent wrote', () => {
    render(<Prose>{'<b>raw</b> text'}</Prose>)

    expect(screen.queryByText('raw')).toBeNull()
    expect(document.body.textContent).toContain('<b>raw</b> text')
  })

  // The author of the text chooses the address of an image. When prose
  // loads it, the browser of each reader sends a request to that host,
  // with data in the address. A mail client that blocks remote images
  // uses the same rule. An image that must show is an `image` block.
  it('shows an image as a link to its address and loads no image', () => {
    render(<Prose>{'![a](https://example.com/p.png?d=secret)'}</Prose>)

    expect(document.querySelector('img')).toBeNull()
    expect(screen.getByRole('link', { name: 'a' }).getAttribute('href')).toBe(
      'https://example.com/p.png?d=secret',
    )
  })

  // A link cannot hold a second link, so an image in a link, such as
  // a badge, is its alt text in the link that the author wrote.
  it('shows an image in a link as its alt text in that link', () => {
    render(
      <Prose>{'[![build](https://example.com/badge.png)](https://example.com/ci)'}</Prose>,
    )

    expect(document.querySelector('img')).toBeNull()
    const links = screen.getAllByRole('link')
    expect(links).toHaveLength(1)
    expect(links[0].textContent).toBe('build')
    expect(links[0].getAttribute('href')).toBe('https://example.com/ci')
  })

  it('names an image that has no alt text "image"', () => {
    render(
      <Prose>
        {'![](https://example.com/p.png)\n\n[![](https://example.com/badge.png)](https://example.com/ci)'}
      </Prose>,
    )

    expect(document.querySelector('img')).toBeNull()
    const links = screen.getAllByRole('link', { name: 'image' })
    expect(links.map((link) => link.getAttribute('href'))).toEqual([
      'https://example.com/p.png',
      'https://example.com/ci',
    ])
  })

  // The link target of an image passes the same address check as the
  // target of each other link.
  it('gives an image with an unsafe address an empty link target', () => {
    render(<Prose>{'![a](javascript:alert(1)) [b](javascript:alert(1))'}</Prose>)

    expect(document.querySelector('img')).toBeNull()
    const anchors = [...document.querySelectorAll('a')]
    expect(anchors.map((anchor) => [anchor.textContent, anchor.getAttribute('href')])).toEqual([
      ['a', ''],
      ['b', ''],
    ])
  })
})
