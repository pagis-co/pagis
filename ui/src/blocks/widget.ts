// The MCP Apps host (ADR-0016). The desk is the Host of the
// extension: a Widget page is untrusted content an agent published, so
// the desk never frames it directly. It frames the daemon's sandbox
// proxy on a second origin, and the proxy loads the page into an inner
// frame at an opaque origin of its own. Nothing in this module touches
// the DOM, so the whole protocol is testable on its own; `WidgetBlock`
// owns the frame and the fetches.
//
// The rules this module holds:
//
//   - The host and the sandbox have different origins. The daemon runs
//     on the loopback address, so the two loopback names it answers to,
//     `localhost` and `127.0.0.1`, are the two origins. A desk that is
//     not on the loopback has no second origin and renders no widget.
//   - The host sends nothing into the view before the view's
//     `ui/notifications/initialized`.
//   - `tools/call`, `resources/read` and `ui/message` go to the
//     daemon's JSON-RPC route, which is the only authority over them.
//     Everything else the view asks for is method-not-found.

/** The dated revision of the extension this host speaks. */
export const WIDGET_PROTOCOL_VERSION = '2026-01-26'

/** The `sandbox` attribute of the proxy frame that the desk holds. The
 *  proxy keeps its origin, because it needs that origin for its
 *  Content-Security-Policy header and for the check in `fromSandbox`.
 *  Only the daemon's proxy code runs at that origin: the proxy holds the
 *  Widget page in an inner frame at an opaque origin, and it sets the
 *  sandbox of that frame itself. */
export const WIDGET_SANDBOX = 'allow-scripts allow-same-origin'

/** Live widget frames in one conversation view (ADR-0016). */
export const MAX_LIVE_WIDGETS = 20

/** The tallest a widget grows before it scrolls inside its frame. */
export const MAX_WIDGET_HEIGHT = 640

const PROXY_READY = 'ui/notifications/sandbox-proxy-ready'
const RESOURCE_READY = 'ui/notifications/sandbox-resource-ready'
const INITIALIZED = 'ui/notifications/initialized'
const SIZE_CHANGED = 'ui/notifications/size-changed'
const TOOL_INPUT = 'ui/notifications/tool-input'
const TOOL_RESULT = 'ui/notifications/tool-result'
const TEARDOWN = 'ui/resource-teardown'

const METHOD_NOT_FOUND = -32601
const INVALID_PARAMS = -32602

export interface JsonRpcError {
  code: number
  message: string
}

/** One frame of the view's JSON-RPC, in either direction. */
export interface JsonRpcMessage {
  jsonrpc: '2.0'
  id?: unknown
  method?: string
  params?: unknown
  result?: unknown
  error?: JsonRpcError
}

/** What the daemon's JSON-RPC route answered: one of the two, never
 *  both. */
export type RpcOutcome = { result: unknown } | { error: JsonRpcError }

/** The context the host gives the view (ADR-0016: the theme and the
 *  container). The desk is light-only, so the theme is a constant. */
export interface HostContext {
  theme: 'light' | 'dark'
  displayMode: 'inline'
  availableDisplayModes: readonly string[]
  containerDimensions: { width: number; maxHeight: number }
}

export function hostContext(containerWidth: number): HostContext {
  return {
    theme: 'light',
    displayMode: 'inline',
    availableDisplayModes: ['inline'],
    containerDimensions: {
      width: containerWidth,
      maxHeight: MAX_WIDGET_HEIGHT,
    },
  }
}

/** The other loopback name of the same daemon. */
const OTHER_LOOPBACK: Record<string, string> = {
  localhost: '127.0.0.1',
  '127.0.0.1': 'localhost',
}

/** The origin the sandbox proxy is framed from, or null when the desk
 *  has no second origin. The extension requires the two to differ, and
 *  a widget that cannot be isolated is not rendered at all. */
export function sandboxOrigin(location: {
  protocol: string
  hostname: string
  port: string
}): string | null {
  const other = OTHER_LOOPBACK[location.hostname]
  if (other === undefined) return null
  const port = location.port === '' ? '' : `:${location.port}`
  return `${location.protocol}//${other}${port}`
}

/** The proxy that frames one Widget. The daemon serves it under that
 *  Widget's own Content-Security-Policy.
 *
 *  The workspace is in the path because the frame sends no cookie: it
 *  loads from the other loopback host, and the session cookie is
 *  SameSite=Strict. So the daemon reads the tenant from the URL, and a
 *  Widget of one person gets that person's own declared policy. */
export function sandboxUrl(
  origin: string,
  workspaceId: string,
  block: { package: string; version: string; widget: string },
): string {
  const path = [workspaceId, block.package, block.version, block.widget]
    .map(encodeURIComponent)
    .join('/')
  return `${origin}/api/v1/widgets/${path}/sandbox`
}

/** Whether one `message` event came from the sandbox this host owns.
 *  A message from any other frame or origin is not the view's. */
export function fromSandbox(
  event: { source: unknown; origin: string },
  frame: { contentWindow: unknown } | null,
  origin: string,
): boolean {
  return (
    frame !== null &&
    frame.contentWindow !== null &&
    event.source === frame.contentWindow &&
    event.origin === origin
  )
}

/** The answer a `ui/message` carries, in the daemon's shape. The
 *  extension puts the text in `content`; Pagis adds the optional
 *  `value`, the structured half of the answer, beside it. */
export function widgetAnswer(
  params: unknown,
): { text: string; value?: unknown } | null {
  if (typeof params !== 'object' || params === null) return null
  const fields = params as Record<string, unknown>
  const content = fields.content as Record<string, unknown> | undefined
  const text =
    typeof fields.text === 'string'
      ? fields.text
      : typeof content?.text === 'string'
        ? content.text
        : null
  if (text === null) return null
  return 'value' in fields ? { text, value: fields.value } : { text }
}

export interface WidgetBridgeOptions {
  /** The page the proxy loads into the inner frame. */
  html: string
  /** The arguments of the tool call that rendered the view, or null
   *  when the view is no longer live. */
  toolInput: unknown
  /** The data half of the tool result, or null when it is gone. */
  structuredContent: unknown
  /** The author's plain-text projection, which the view reads as the
   *  `content` of its tool result. */
  projection: string
  containerWidth: number
  /** Post one message into the sandbox. */
  post: (message: JsonRpcMessage) => void
  /** Call the daemon's JSON-RPC route for this view. */
  call: (method: string, params: unknown) => Promise<RpcOutcome>
  /** The view reported its own size. */
  onSize?: (size: { width: number; height: number }) => void
}

/** The host half of one widget frame. */
export class WidgetBridge {
  private initialized = false
  private torn = false
  private nextRequestId = 1

  constructor(private readonly options: WidgetBridgeOptions) {}

  /** Fold one message from the sandbox. */
  async receive(message: unknown): Promise<void> {
    if (typeof message !== 'object' || message === null) return
    const frame = message as JsonRpcMessage
    const method = frame.method
    // A frame with no method is the view's answer to a request of ours;
    // the host asks only for a teardown, and waits for nothing.
    if (typeof method !== 'string') return
    if (method === PROXY_READY) {
      this.options.post({
        jsonrpc: '2.0',
        method: RESOURCE_READY,
        params: { html: this.options.html },
      })
      return
    }
    if (method === INITIALIZED) {
      this.begin()
      return
    }
    if (method === SIZE_CHANGED) {
      const size = frame.params as { width?: unknown; height?: unknown } | null
      if (typeof size?.width === 'number' && typeof size.height === 'number') {
        this.options.onSize?.({ width: size.width, height: size.height })
      }
      return
    }
    // Every other notification is read and dropped: a request carries
    // an id, and only a request earns an answer.
    if (frame.id === undefined) return
    const outcome = await this.answer(method, frame.params)
    this.options.post({ jsonrpc: '2.0', id: frame.id, ...outcome })
  }

  /** Tell the view its run is over, once. */
  teardown(reason: string): void {
    if (!this.initialized || this.torn) return
    this.torn = true
    this.options.post({
      jsonrpc: '2.0',
      id: this.nextRequestId++,
      method: TEARDOWN,
      params: { reason },
    })
  }

  /** The view is ready, so the host pushes what the tool call held. */
  private begin(): void {
    if (this.initialized) return
    this.initialized = true
    this.options.post({
      jsonrpc: '2.0',
      method: TOOL_INPUT,
      params: { arguments: this.options.toolInput ?? {} },
    })
    if (this.options.structuredContent === null) return
    this.options.post({
      jsonrpc: '2.0',
      method: TOOL_RESULT,
      params: {
        content: [{ type: 'text', text: this.options.projection }],
        structuredContent: this.options.structuredContent,
        isError: false,
      },
    })
  }

  private async answer(method: string, params: unknown): Promise<RpcOutcome> {
    switch (method) {
      case 'ui/initialize':
        return {
          result: {
            protocolVersion: WIDGET_PROTOCOL_VERSION,
            hostInfo: { name: 'pagis', version: WIDGET_PROTOCOL_VERSION },
            hostCapabilities: { serverTools: {}, serverResources: {} },
            hostContext: hostContext(this.options.containerWidth),
          },
        }
      case 'ping':
        return { result: {} }
      case 'tools/call':
      case 'resources/read':
        return this.options.call(method, params)
      case 'ui/message': {
        const answer = widgetAnswer(params)
        if (answer === null) {
          return {
            error: {
              code: INVALID_PARAMS,
              message: 'ui/message needs the text of the answer',
            },
          }
        }
        return this.options.call('ui/message', answer)
      }
      default:
        return {
          error: {
            code: METHOD_NOT_FOUND,
            message: `a widget cannot call ${method}`,
          },
        }
    }
  }
}
