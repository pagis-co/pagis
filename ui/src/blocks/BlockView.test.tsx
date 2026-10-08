// Block rendering (ADR-0004): markdown renders as markup; image and
// file blocks fetch their bytes through the daemon, and an image block
// shows only a passive image type; a block type
// this build cannot draw — one from a newer daemon, or one the union
// types with no renderer — takes the generic fallback
// instead of breaking the message. The table, form and choice card
// have their own suites.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { fetchArtifactBlob, type ApiClient, type MessageDto } from '../api/client'
import { externalImages } from '../test/images'
import { renderInRouter } from '../test/router'
import { Blocks } from './BlockView'

vi.mock('../api/client', () => ({
  fetchArtifactBlob: vi.fn(),
}))

const api = { GET: vi.fn(async () => ({ data: undefined })) }

beforeEach(() => {
  vi.mocked(fetchArtifactBlob).mockReset()
  URL.createObjectURL = vi.fn(() => 'blob:fake-url')
  URL.revokeObjectURL = vi.fn()
})

const codingSessionBlock = {
  type: 'coding_session',
  coding_session_id: 'cs1',
  harness: 'Claude Code',
  machine: 'Air',
  directory: '/Users/bo/code/app',
  title: 'Fix the login bug',
}

// The wire can carry what the generated union does not describe: a
// block from a newer daemon, with a type or a shape this build does not
// know.
function mount(blocks: unknown) {
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

describe('Blocks', () => {
  it('renders an image block from bytes fetched through the daemon', async () => {
    vi.mocked(fetchArtifactBlob).mockResolvedValue(
      new Blob(['png'], { type: 'image/png' }),
    )

    mount([{ type: 'image', artifact_id: 'A1', alt: 'shot.png' }])

    const image = await waitFor(() => screen.getByAltText('shot.png'))
    expect(image.getAttribute('src')).toBe('blob:fake-url')
    expect(fetchArtifactBlob).toHaveBeenCalledWith('A1')
  })

  // An object URL has the Product App origin, so an SVG or HTML Blob
  // behind one runs its script with the Person's Session when a click
  // on Open shows it.
  it('makes an object URL and an Open link only for a passive image type', async () => {
    const blobs: Record<string, Blob> = {
      PNG: new Blob(['png'], { type: 'image/png' }),
      SVG: new Blob(
        ['<svg xmlns="http://www.w3.org/2000/svg"><script>fetch("/api/v1/requests")</script></svg>'],
        { type: 'image/svg+xml' },
      ),
      HTML: new Blob(['<script>fetch("/api/v1/requests")</script>'], {
        type: 'text/html',
      }),
    }
    vi.mocked(fetchArtifactBlob).mockImplementation(async (id) => blobs[id])

    mount([
      { type: 'image', artifact_id: 'PNG', alt: 'shot.png' },
      { type: 'image', artifact_id: 'SVG', alt: 'chart.svg' },
      { type: 'image', artifact_id: 'HTML', alt: 'page.html' },
    ])
    // Each block has its bytes when it offers Save.
    await waitFor(() =>
      expect(screen.getAllByRole('button', { name: 'Save' })).toHaveLength(3),
    )

    expect(URL.createObjectURL).toHaveBeenCalledTimes(1)
    expect(URL.createObjectURL).toHaveBeenCalledWith(blobs.PNG)
    const open = screen.getAllByRole('link', { name: 'Open' })
    expect(open).toHaveLength(1)
    expect(open[0].getAttribute('href')).toBe('blob:fake-url')
    expect(screen.getByAltText('shot.png').getAttribute('src')).toBe('blob:fake-url')
    expect(screen.queryByAltText('chart.svg')).toBeNull()
    expect(screen.queryByAltText('page.html')).toBeNull()
    expect(screen.getAllByText('No preview for this file type')).toHaveLength(2)
  })

  it('shows a placeholder when the image bytes cannot load', async () => {
    vi.mocked(fetchArtifactBlob).mockRejectedValue(new Error('gone'))

    mount([{ type: 'image', artifact_id: 'A1' }])

    await waitFor(() =>
      expect(screen.getByTestId('image-block').textContent).toContain(
        'Image unavailable',
      ),
    )
  })

  it('renders a file block with its name, its size and one Save', () => {
    mount([
      { type: 'file', artifact_id: 'A2', name: 'report.pdf', mime: 'application/pdf', size_bytes: 188416 },
    ])

    const block = screen.getByTestId('file-block')
    expect(block.textContent).toContain('report.pdf')
    expect(block.textContent).toContain('application/pdf · 184 KB')
    expect(screen.getAllByRole('button', { name: 'Save' })).toHaveLength(1)
    // Rendering alone never fetches; the download is on demand.
    expect(fetchArtifactBlob).not.toHaveBeenCalled()
  })

  it('renders a markdown block as markup', () => {
    mount([{ type: 'markdown', text: '**bold** word' }])
    const bold = screen.getByText('bold')
    expect(bold.tagName).toBe('STRONG')
  })

  // The text that an Agent writes can name an image at any host. The
  // message loads none of them: the image is a link.
  it('loads no image that a markdown block names', () => {
    mount([{ type: 'markdown', text: 'The chart: ![a](https://example.com/p.png?d=secret)' }])

    expect(externalImages()).toEqual([])
    expect(screen.getByRole('link', { name: 'a' }).getAttribute('href')).toBe(
      'https://example.com/p.png?d=secret',
    )
  })

  // GitHub Flavored Markdown: an agent writes a table in prose
  // even though `add_block` offers a table block, and a pipe table that
  // renders as one run of pipes is unreadable.
  it('renders a markdown table as a grid', () => {
    mount([
      {
        type: 'markdown',
        text: [
          '| Plan | Provider | Price |',
          '|---|---|---:|',
          '| Fibre 900 | Cascade Link | $89 |',
        ].join('\n'),
      },
    ])
    expect(screen.getByRole('table')).toBeDefined()
    expect(screen.getByRole('columnheader', { name: 'Plan' })).toBeDefined()
    expect(screen.getByRole('cell', { name: 'Cascade Link' })).toBeDefined()
  })

  it('renders a progress block as the daemon composed it', () => {
    mount([{ type: 'progress', run_id: 'run-1', text: 'Running `git status`…' }])
    const block = screen.getByTestId('progress-block')
    expect(block.textContent).toContain('Running')
    // The daemon writes the command as code; the line renders as markup.
    expect(screen.getByText('git status').tagName).toBe('CODE')
  })

  it('renders the generic fallback for an unknown block type', () => {
    mount([{ type: 'hologram', text: 'Beam it up?' }])
    const fallback = screen.getByTestId('unknown-block')
    expect(fallback.textContent).toContain('hologram')
    expect(fallback.textContent).toContain('Beam it up?')
  })

  it('renders the fallback for a typed block that has no renderer', () => {
    // `screen` is in the union but has no renderer, so it takes the
    // fallback, like an unknown type.
    mount([{ type: 'screen', agent_id: 'ag1' }])
    const fallback = screen.getByTestId('unknown-block')
    expect(fallback.textContent).toContain('screen')
  })

  it('renders a coding session block as the session card', async () => {
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    })
    // The record never arrives, so the card waits for it.
    const waiting = { GET: vi.fn(() => new Promise(() => {})) }
    renderInRouter(
      <QueryClientProvider client={queryClient}>
        <Blocks
          blocks={[codingSessionBlock] as MessageDto['blocks']}
          api={waiting as unknown as ApiClient}
        />
      </QueryClientProvider>,
    )
    expect(await screen.findByText('Opening the coding session…')).toBeTruthy()
    expect(waiting.GET).toHaveBeenCalledWith(
      '/api/v1/coding-sessions/{coding_session_id}',
      { params: { path: { coding_session_id: 'cs1' } } },
    )
    expect(screen.queryByTestId('unknown-block')).toBeNull()
  })

  it('renders the fallback for a coding session block with no session id', () => {
    mount([{ ...codingSessionBlock, coding_session_id: undefined }])
    const fallback = screen.getByTestId('unknown-block')
    expect(fallback.textContent).toContain('coding_session')
  })

  it('renders a mail block as its strip', () => {
    mount([
      {
        type: 'mail',
        direction: 'inbound',
        mailbox: 'ada@example.com',
        message_id: 'INBOX:12',
        counterpart: 'care@clinic.test',
        subject: 'Your appointment',
        trust_tier: 'unknown',
      },
    ])
    expect(screen.getByTestId('mail-strip').textContent).toContain(
      'Mail from care@clinic.test',
    )
  })

  it('renders the fallback for a mail block with no message id', () => {
    mount([{ type: 'mail', direction: 'inbound', mailbox: 'ada@example.com' }])
    expect(screen.getByTestId('unknown-block')).toBeTruthy()
  })

  it('renders the fallback for a malformed approval card', () => {
    // approval_card without request_id is not renderable as a card.
    mount([{ type: 'approval_card', text: 'Approve the thing?' }])
    expect(screen.getByTestId('unknown-block')).toBeTruthy()
  })

  it('renders the fallback for a widget block with no tool call', () => {
    // Without a tool call there is no view to render. The frame
    // itself has its own suite, because it needs a second origin.
    mount([{ type: 'widget', package: 'weather', widget: 'forecast-card' }])
    expect(screen.getByTestId('unknown-block')).toBeTruthy()
  })

  it('renders the fallback for a block with no type', () => {
    mount([{ text: 'shapeless' }])
    expect(screen.getByTestId('unknown-block')).toBeTruthy()
  })

  it('renders nothing for a non-array blocks value', () => {
    const { container } = mount('not blocks')
    expect(container.innerHTML).toBe('')
  })
})
