// The `widget` block (ADR-0016): the sandbox frame on the second
// origin, the projection everywhere the frame cannot run, and the cap
// on how many frames one conversation holds.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { ApiClient } from '../api/client'
import { useWidgetFrames, useWidgetTeardowns } from '../state/stores'
import { WidgetBlock } from './WidgetBlock'
import { MAX_LIVE_WIDGETS, sandboxOrigin } from './widget'

vi.mock('../api/client', async () => {
  const actual =
    await vi.importActual<typeof import('../api/client')>('../api/client')
  return {
    ...actual,
    fetchWidgetPage: vi.fn(async () => '<!doctype html><p>the page</p>'),
  }
})

vi.mock('./widget', async () => {
  const actual = await vi.importActual<typeof import('./widget')>('./widget')
  return { ...actual, sandboxOrigin: vi.fn(actual.sandboxOrigin) }
})

const { fetchWidgetPage } = await import('../api/client')

function block(toolCallId: string) {
  return {
    type: 'widget' as const,
    package: 'weather',
    version: 'v1',
    widget: 'forecast-card',
    tool_call_id: toolCallId,
    text: 'Tomorrow reaches 21 degrees.',
  }
}

function stubApi(): ApiClient {
  return {
    // The block reads the channels too, for the tenant the sandbox URL
    // names.
    GET: vi.fn(async (path: string) => ({
      data:
        path === '/api/v1/channels'
          ? { items: [{ id: 'ch1', workspace_id: 'ws-1' }] }
          : {
              tool_call_id: 'call_1',
              package: 'weather',
              version: 'v1',
              widget: 'forecast-card',
              tool_input: { city: 'Lisbon' },
              structured_content: { high: 21 },
            },
    })),
    POST: vi.fn(async () => ({ data: { jsonrpc: '2.0', id: 0, result: {} } })),
  } as unknown as ApiClient
}

function draw(toolCallIds: readonly string[]) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  const api = stubApi()
  return render(
    <QueryClientProvider client={client}>
      {toolCallIds.map((id) => (
        <WidgetBlock key={id} block={block(id)} api={api} />
      ))}
    </QueryClientProvider>,
  )
}

beforeEach(() => {
  vi.mocked(fetchWidgetPage).mockResolvedValue('<!doctype html><p>the page</p>')
  vi.mocked(sandboxOrigin).mockImplementation((location) =>
    location.hostname === 'nowhere' ? null : 'http://127.0.0.1:4400',
  )
  useWidgetFrames.setState({ live: [] })
  useWidgetTeardowns.setState({ torn: {} })
})

describe('the widget block', () => {
  it('frames the sandbox proxy on the second origin', async () => {
    draw(['call_1'])
    const frame = await screen.findByTitle('weather/forecast-card')
    expect(frame.getAttribute('src')).toBe(
      'http://127.0.0.1:4400/api/v1/widgets/ws-1/weather/v1/forecast-card/sandbox',
    )
    expect(frame.getAttribute('sandbox')).toBe('allow-scripts allow-same-origin')
  })

  it('tells the frame when the run tore its view down', async () => {
    draw(['call_1'])
    const frame = (await screen.findByTitle(
      'weather/forecast-card',
    )) as HTMLIFrameElement
    const view = frame.contentWindow
    if (view === null) throw new Error('the frame has no window')
    const posted: { method?: string }[] = []
    view.postMessage = ((message: { method?: string }) => {
      posted.push(message)
    }) as typeof view.postMessage

    // The view says it is ready, so the host pushes what the call held.
    window.dispatchEvent(
      new MessageEvent('message', {
        data: { jsonrpc: '2.0', method: 'ui/notifications/initialized' },
        origin: 'http://127.0.0.1:4400',
        source: view,
      }),
    )
    await waitFor(() =>
      expect(posted.map((message) => message.method)).toEqual([
        'ui/notifications/tool-input',
        'ui/notifications/tool-result',
      ]),
    )

    act(() => useWidgetTeardowns.getState().tearDown(['call_1']))
    await waitFor(() =>
      expect(posted[posted.length - 1].method).toBe('ui/resource-teardown'),
    )
  })

  it('drops a message that is not from its own sandbox', async () => {
    draw(['call_1'])
    const frame = (await screen.findByTitle(
      'weather/forecast-card',
    )) as HTMLIFrameElement
    const view = frame.contentWindow
    if (view === null) throw new Error('the frame has no window')
    const posted: unknown[] = []
    view.postMessage = ((message: unknown) => {
      posted.push(message)
    }) as typeof view.postMessage

    window.dispatchEvent(
      new MessageEvent('message', {
        data: { jsonrpc: '2.0', method: 'ui/notifications/initialized' },
        origin: 'http://evil.example',
        source: view,
      }),
    )
    await Promise.resolve()
    expect(posted).toEqual([])
  })

  it('shows the projection while the page is unavailable', async () => {
    vi.mocked(fetchWidgetPage).mockRejectedValue(new Error('gone'))
    draw(['call_1'])
    expect(
      await screen.findByText('Tomorrow reaches 21 degrees.'),
    ).toBeDefined()
    expect(screen.queryByTestId('widget-block')).toBeNull()
  })

  it('shows the projection when the desk has no second origin', async () => {
    vi.mocked(sandboxOrigin).mockReturnValue(null)
    draw(['call_1'])
    expect(await screen.findByTestId('widget-projection')).toBeDefined()
    expect(
      screen.getByText(
        'This desk cannot isolate a widget, so it shows the summary.',
      ),
    ).toBeDefined()
  })

  it('holds no more than the cap of live frames', async () => {
    const ids = Array.from(
      { length: MAX_LIVE_WIDGETS + 1 },
      (_, index) => `call_${index}`,
    )
    draw(ids)
    await waitFor(() =>
      expect(screen.getAllByTestId('widget-block')).toHaveLength(
        MAX_LIVE_WIDGETS,
      ),
    )
    // The oldest block of the conversation gave its slot up.
    expect(screen.getAllByTestId('widget-projection')).toHaveLength(1)
    expect(useWidgetFrames.getState().live).not.toContain('call_0')
  })
})
