// The MCP Apps host half (ADR-0016): the second origin, the
// message forwarding, and the one path from a view to the daemon.

import { describe, expect, it, vi } from 'vitest'

import {
  WidgetBridge,
  fromSandbox,
  hostContext,
  sandboxOrigin,
  sandboxUrl,
  widgetAnswer,
  type JsonRpcMessage,
  type RpcOutcome,
} from './widget'

function bridge(
  overrides: Partial<{
    structuredContent: unknown
    call: (method: string, params: unknown) => Promise<RpcOutcome>
    onSize: (size: { width: number; height: number }) => void
  }> = {},
) {
  const sent: JsonRpcMessage[] = []
  const host = new WidgetBridge({
    html: '<!doctype html><p>the page</p>',
    toolInput: { city: 'Lisbon' },
    structuredContent:
      'structuredContent' in overrides
        ? overrides.structuredContent
        : { high: 21 },
    projection: 'Tomorrow reaches 21 degrees.',
    containerWidth: 400,
    post: (message) => sent.push(message),
    call: overrides.call ?? (async () => ({ result: {} })),
    onSize: overrides.onSize,
  })
  return { host, sent }
}

const initialized: JsonRpcMessage = {
  jsonrpc: '2.0',
  method: 'ui/notifications/initialized',
}

describe('the second origin', () => {
  it('is the other loopback name of the same daemon', () => {
    expect(
      sandboxOrigin({ protocol: 'http:', hostname: 'localhost', port: '4400' }),
    ).toBe('http://127.0.0.1:4400')
    expect(
      sandboxOrigin({ protocol: 'http:', hostname: '127.0.0.1', port: '4400' }),
    ).toBe('http://localhost:4400')
  })

  it('keeps the default port implicit', () => {
    expect(
      sandboxOrigin({ protocol: 'http:', hostname: 'localhost', port: '' }),
    ).toBe('http://127.0.0.1')
  })

  it('does not exist for a desk that is not on the loopback', () => {
    expect(
      sandboxOrigin({ protocol: 'https:', hostname: 'desk.example', port: '' }),
    ).toBeNull()
  })

  // The tenant is in the path because the frame sends no cookie.
  it('names the tenant and the widget proxy of one Version', () => {
    expect(
      sandboxUrl('http://127.0.0.1:4400', 'ws-1', {
        package: 'weather',
        version: 'v1',
        widget: 'forecast card',
      }),
    ).toBe(
      'http://127.0.0.1:4400/api/v1/widgets/ws-1/weather/v1/forecast%20card/sandbox',
    )
  })
})

describe('a message from the sandbox', () => {
  const frame = { contentWindow: { name: 'the frame' } }

  it('comes from the frame this block holds, on the sandbox origin', () => {
    expect(
      fromSandbox(
        { source: frame.contentWindow, origin: 'http://127.0.0.1:4400' },
        frame,
        'http://127.0.0.1:4400',
      ),
    ).toBe(true)
  })

  it('is dropped from another origin or another frame', () => {
    expect(
      fromSandbox(
        { source: frame.contentWindow, origin: 'http://evil.example' },
        frame,
        'http://127.0.0.1:4400',
      ),
    ).toBe(false)
    expect(
      fromSandbox(
        { source: { other: true }, origin: 'http://127.0.0.1:4400' },
        frame,
        'http://127.0.0.1:4400',
      ),
    ).toBe(false)
    expect(
      fromSandbox(
        { source: frame.contentWindow, origin: 'http://127.0.0.1:4400' },
        null,
        'http://127.0.0.1:4400',
      ),
    ).toBe(false)
  })
})

describe('the widget bridge', () => {
  // The proxy sets the sandbox of the page itself. The host sends no
  // sandbox value, because the isolation of the page must not depend
  // on a value that crosses the frame boundary.
  it('hands the page alone to the proxy when the proxy is ready', async () => {
    const { host, sent } = bridge()
    await host.receive({
      jsonrpc: '2.0',
      method: 'ui/notifications/sandbox-proxy-ready',
      params: {},
    })
    expect(sent).toEqual([
      {
        jsonrpc: '2.0',
        method: 'ui/notifications/sandbox-resource-ready',
        params: { html: '<!doctype html><p>the page</p>' },
      },
    ])
  })

  it('answers ui/initialize with the host context', async () => {
    const { host, sent } = bridge()
    await host.receive({ jsonrpc: '2.0', id: 1, method: 'ui/initialize' })
    expect(sent).toHaveLength(1)
    expect(sent[0].id).toBe(1)
    const result = sent[0].result as { hostContext: unknown }
    expect(result.hostContext).toEqual(hostContext(400))
  })

  it('sends nothing into the view before the view is initialized', async () => {
    const { host, sent } = bridge()
    await host.receive({ jsonrpc: '2.0', id: 1, method: 'ping' })
    expect(sent.every((message) => message.method === undefined)).toBe(true)
  })

  it('pushes the tool input and then the tool result', async () => {
    const { host, sent } = bridge()
    await host.receive(initialized)
    expect(sent).toEqual([
      {
        jsonrpc: '2.0',
        method: 'ui/notifications/tool-input',
        params: { arguments: { city: 'Lisbon' } },
      },
      {
        jsonrpc: '2.0',
        method: 'ui/notifications/tool-result',
        params: {
          content: [
            { type: 'text', text: 'Tomorrow reaches 21 degrees.' },
          ],
          structuredContent: { high: 21 },
          isError: false,
        },
      },
    ])
  })

  it('pushes no tool result when the view is no longer live', async () => {
    const { host, sent } = bridge({ structuredContent: null })
    await host.receive(initialized)
    expect(sent.map((message) => message.method)).toEqual([
      'ui/notifications/tool-input',
    ])
  })

  it('initializes once', async () => {
    const { host, sent } = bridge()
    await host.receive(initialized)
    await host.receive(initialized)
    expect(sent).toHaveLength(2)
  })

  it('routes a tool call to the daemon and returns its result', async () => {
    const call = vi.fn(async () => ({ result: { isError: false } }))
    const { host, sent } = bridge({ call })
    await host.receive({
      jsonrpc: '2.0',
      id: 7,
      method: 'tools/call',
      params: { name: 'page', arguments: { page: 2 } },
    })
    expect(call).toHaveBeenCalledWith('tools/call', {
      name: 'page',
      arguments: { page: 2 },
    })
    expect(sent[0]).toEqual({
      jsonrpc: '2.0',
      id: 7,
      result: { isError: false },
    })
  })

  it('returns the daemon error as the answer', async () => {
    const call = vi.fn(async () => ({
      error: { code: -32602, message: 'a widget of weather cannot read that' },
    }))
    const { host, sent } = bridge({ call })
    await host.receive({
      jsonrpc: '2.0',
      id: 8,
      method: 'resources/read',
      params: { uri: 'ui://banking/balance' },
    })
    expect(sent[0].error).toEqual({
      code: -32602,
      message: 'a widget of weather cannot read that',
    })
  })

  it('answers a method no widget may call with method-not-found', async () => {
    const { host, sent } = bridge()
    for (const method of [
      'tools/list',
      'ui/update-model-context',
      'ui/open-link',
    ]) {
      await host.receive({ jsonrpc: '2.0', id: method, method })
    }
    expect(sent.map((message) => message.error?.code)).toEqual([
      -32601, -32601, -32601,
    ])
  })

  it('leaves a notification unanswered', async () => {
    const { host, sent } = bridge()
    await host.receive({ jsonrpc: '2.0', method: 'notifications/message' })
    expect(sent).toEqual([])
  })

  it('reads the size the view reports', async () => {
    const onSize = vi.fn()
    const { host } = bridge({ onSize })
    await host.receive({
      jsonrpc: '2.0',
      method: 'ui/notifications/size-changed',
      params: { width: 400, height: 210 },
    })
    expect(onSize).toHaveBeenCalledWith({ width: 400, height: 210 })
  })

  it('tells the frame once that its run is over', async () => {
    const { host, sent } = bridge()
    await host.receive(initialized)
    host.teardown('the run ended')
    host.teardown('the run ended')
    const teardowns = sent.filter(
      (message) => message.method === 'ui/resource-teardown',
    )
    expect(teardowns).toHaveLength(1)
    expect(teardowns[0].params).toEqual({ reason: 'the run ended' })
    expect(teardowns[0].id).toBeDefined()
  })

  it('tells a frame that never initialized nothing', () => {
    const { host, sent } = bridge()
    host.teardown('the run ended')
    expect(sent).toEqual([])
  })
})

describe('the answer of a ui/message', () => {
  it('reads the text the extension puts in content', () => {
    expect(
      widgetAnswer({ role: 'user', content: { type: 'text', text: 'row two' } }),
    ).toEqual({ text: 'row two' })
  })

  it('carries the structured half beside the text', () => {
    expect(widgetAnswer({ text: 'row two', value: { row: 2 } })).toEqual({
      text: 'row two',
      value: { row: 2 },
    })
  })

  it('does not exist without text', () => {
    expect(widgetAnswer({ value: { row: 2 } })).toBeNull()
    expect(widgetAnswer(null)).toBeNull()
  })

  it('refuses a ui/message the daemon would refuse', async () => {
    const call = vi.fn(async () => ({ result: {} }))
    const { host, sent } = bridge({ call })
    await host.receive({ jsonrpc: '2.0', id: 3, method: 'ui/message', params: {} })
    expect(call).not.toHaveBeenCalled()
    expect(sent[0].error?.code).toBe(-32602)
  })

  it('sends the daemon the two fields it validates', async () => {
    const call = vi.fn(async () => ({ result: {} }))
    const { host } = bridge({ call })
    await host.receive({
      jsonrpc: '2.0',
      id: 4,
      method: 'ui/message',
      params: { content: { type: 'text', text: 'row two' }, value: { row: 2 } },
    })
    expect(call).toHaveBeenCalledWith('ui/message', {
      text: 'row two',
      value: { row: 2 },
    })
  })
})
