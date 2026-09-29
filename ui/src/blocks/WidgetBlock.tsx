// The `widget` block (ADR-0016): one Widget of a Software
// Package, rendered in the frame the MCP Apps extension asks for.
//
// The block carries no HTML and no data. The page comes from the daemon
// by package, Version and Widget name, and the data half from the live
// view by tool call id, so an older message renders the Version it
// names and its projection alone.
//
// The desk frames the daemon's sandbox proxy on a second origin, never
// the page itself, and hands the page to the proxy over `postMessage`.
// A desk with no second origin renders the projection instead: a widget
// that cannot be isolated is not rendered at all.

import { useEffect, useRef, useState } from 'react'
import { Package } from 'lucide-react'

import type { ApiClient } from '../api/client'
import type { components } from '../api/schema'
import { useChannels, useWidgetPage, useWidgetView } from '../queries'
import { Card, CardBody, CardFooter, CardHeader } from './Card'
import {
  selectWidgetLive,
  selectWidgetTorn,
  useWidgetFrames,
  useWidgetTeardowns,
} from '../state/stores'
import {
  MAX_WIDGET_HEIGHT,
  WIDGET_SANDBOX,
  WidgetBridge,
  fromSandbox,
  sandboxOrigin,
  sandboxUrl,
  type JsonRpcMessage,
  type RpcOutcome,
} from './widget'

type WidgetBlockDto = Extract<
  components['schemas']['KnownBlock'],
  { type: 'widget' }
>

/** The height a widget starts at, before it reports its own. */
const START_HEIGHT = 320

/** The daemon failed the call, which is not a JSON-RPC outcome. */
const INTERNAL_ERROR = -32603

/** The header names the Widget, its package and its Version. */
function WidgetHeader({ block }: { block: WidgetBlockDto }) {
  return (
    <CardHeader
      icon={Package}
      act={block.widget}
      place={`${block.package} · v${block.version} · sandboxed`}
    />
  )
}

/** The projection, which is what a reader gets when the frame cannot
 *  run. The text is the package author's, so it is labelled. */
function Projection({
  block,
  note,
}: {
  block: WidgetBlockDto
  note?: string
}) {
  return (
    <Card className="widget-block widget-projection" data-testid="widget-projection">
      <WidgetHeader block={block} />
      <CardBody>
        <p>{block.text}</p>
        {note !== undefined && <p className="widget-note">{note}</p>}
      </CardBody>
    </Card>
  )
}

export function WidgetBlock({
  block,
  api,
}: {
  block: WidgetBlockDto
  api: ApiClient
}) {
  const toolCallId = block.tool_call_id
  const claim = useWidgetFrames((state) => state.claim)
  const release = useWidgetFrames((state) => state.release)
  const live = useWidgetFrames(selectWidgetLive(toolCallId))
  const torn = useWidgetTeardowns(selectWidgetTorn(toolCallId))
  const [origin] = useState(() => sandboxOrigin(window.location))
  // The frame sends no cookie, so the daemon reads the tenant from the
  // sandbox URL. Every Channel of this desk is the signed-in
  // person's, so any of them names it.
  const channels = useChannels(api)
  const workspaceId = channels.data?.[0]?.workspace_id
  const [height, setHeight] = useState(START_HEIGHT)
  const frameRef = useRef<HTMLIFrameElement | null>(null)
  const bridgeRef = useRef<WidgetBridge | null>(null)

  // One slot of the conversation's frame budget, held while the block
  // is on screen.
  useEffect(() => {
    claim(toolCallId)
    return () => release(toolCallId)
  }, [claim, release, toolCallId])

  const framed = live && origin !== null
  const page = useWidgetPage(
    block.package,
    block.version,
    block.widget,
    framed,
  )
  // The data half is gone once the Run ended. That is not a failure:
  // the page still renders, without its data.
  const view = useWidgetView(api, toolCallId, framed)
  const html = page.data
  const settled = view.isSuccess || view.isError

  useEffect(() => {
    if (html === undefined || origin === null || !settled) return
    const post = (message: JsonRpcMessage) => {
      frameRef.current?.contentWindow?.postMessage(message, origin)
    }
    const call = async (
      method: string,
      params: unknown,
    ): Promise<RpcOutcome> => {
      const { data } = await api.POST('/api/v1/widgets/{tool_call_id}/rpc', {
        params: { path: { tool_call_id: toolCallId } },
        body: { jsonrpc: '2.0', id: 0, method, params },
      })
      if (data === undefined) {
        return {
          error: { code: INTERNAL_ERROR, message: 'this widget is not live' },
        }
      }
      if (data.error != null) return { error: data.error }
      return { result: data.result ?? {} }
    }
    const bridge = new WidgetBridge({
      html,
      toolInput: view.data?.tool_input ?? null,
      structuredContent: view.data?.structured_content ?? null,
      projection: block.text,
      containerWidth: frameRef.current?.clientWidth ?? 0,
      post,
      call,
      onSize: (size) =>
        setHeight(Math.min(Math.max(size.height, 1), MAX_WIDGET_HEIGHT)),
    })
    bridgeRef.current = bridge
    const listener = (event: MessageEvent) => {
      if (!fromSandbox(event, frameRef.current, origin)) return
      void bridge.receive(event.data)
    }
    window.addEventListener('message', listener)
    return () => {
      window.removeEventListener('message', listener)
      bridgeRef.current = null
    }
  }, [api, block.text, html, origin, settled, toolCallId, view.data])

  // The Run ended, so the view is gone: tell the frame before it goes.
  useEffect(() => {
    if (torn) bridgeRef.current?.teardown('the run ended')
  }, [torn])

  if (origin === null) {
    return (
      <Projection
        block={block}
        note="This desk cannot isolate a widget, so it shows the summary."
      />
    )
  }
  if (workspaceId === undefined) return <Projection block={block} />
  if (!live || page.isError) return <Projection block={block} />
  if (html === undefined || !settled) return <Projection block={block} />

  return (
    <Card className="widget-block" data-testid="widget-block">
      <WidgetHeader block={block} />
      <iframe
        ref={frameRef}
        className="widget-frame"
        title={`${block.package}/${block.widget}`}
        src={sandboxUrl(origin, workspaceId, block)}
        sandbox={WIDGET_SANDBOX}
        style={{ height: `${height}px` }}
      />
      <CardFooter className="widget-note">
        A Software Package page. It cannot read the thread; it gets its
        own data by tool call.
      </CardFooter>
    </Card>
  )
}
