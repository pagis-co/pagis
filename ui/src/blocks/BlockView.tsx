// Rich message blocks (ADR-0004): the daemon owns the union, the
// generated `KnownBlock` type is its only definition here, and one
// renderer serves each type that has one. A block that views a
// Request — approval card, form, choice card — carries only its
// identifier and its display fields; the state and the submitted
// answer come from the row. The fallback is mandatory:
// a block type this build does not know renders generically
// instead of breaking the message. Image and file blocks fetch
// their bytes through the daemon with the session cookie, never from
// storage directly. An image block shows only a passive image type.
// Every framed block composes the Card.

import { useEffect, useState } from 'react'
import { Clock, File as FileIcon } from 'lucide-react'

import type { ApiClient } from '../api/client'
import { fetchArtifactBlob } from '../api/client'
import type { components } from '../api/schema'
import { Button } from '../primitives'
import { Prose } from '../prose'
import { ApprovalCard } from './ApprovalCard'
import { Card, CardBody, CardFooter } from './Card'
import { CallBlock } from './CallBlock'
import { CodingSessionBlock } from './CodingSessionBlock'
import { ChoiceCard } from './ChoiceCard'
import { MailBlock } from './MailBlock'
import { RequestForm } from './RequestForm'
import { TableBlock } from './TableBlock'
import { WidgetBlock } from './WidgetBlock'

type KnownBlock = components['schemas']['KnownBlock']
type BlockOfType<T extends KnownBlock['type']> = Extract<KnownBlock, { type: T }>

/** What the wire carries: a known block, or a block from a newer daemon. */
type WireBlock = KnownBlock | { type: string; [key: string]: unknown }

const KNOWN_TYPES: readonly KnownBlock['type'][] = [
  'markdown',
  'table',
  'form',
  'choice_card',
  'approval_card',
  'progress',
  'image',
  'file',
  'screen',
  'call',
  'widget',
  'mail',
  'coding_session',
]

function isBlockShaped(value: unknown): value is WireBlock {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as { type?: unknown }).type === 'string'
  )
}

function isKnown(block: WireBlock): block is KnownBlock {
  return (KNOWN_TYPES as readonly string[]).includes(block.type)
}

/** The generic fallback: name the type, show the text summary if any. */
function UnknownBlock({ block }: { block: { type: string } }) {
  const summary = (block as { text?: unknown }).text
  return (
    <Card className="block-unknown" data-testid="unknown-block">
      <CardBody>
        <span className="block-unknown-type">Unsupported block: {block.type}</span>
        {typeof summary === 'string' && <p>{summary}</p>}
      </CardBody>
    </Card>
  )
}

/** Hand the reader a file under its own name. */
function saveBlob(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = name
  anchor.click()
  URL.revokeObjectURL(url)
}

/** The image types that a browser shows and never runs. The daemon
 *  serves these inline and every other type as a download. */
const PASSIVE_IMAGE_TYPES: ReadonlySet<string> = new Set([
  'image/png',
  'image/jpeg',
  'image/gif',
  'image/webp',
])

function isPassiveImage(blob: Blob): boolean {
  const essence = blob.type.split(';')[0].trim().toLowerCase()
  return PASSIVE_IMAGE_TYPES.has(essence)
}

/** An image artifact, fetched through the daemon into an object URL.
 *  The alt text is the caption; one Save hands the bytes over. An
 *  object URL has the Product App origin, so the block makes one only
 *  for a passive image: an SVG or HTML document behind it would run
 *  its script with the Person's Session when Open shows it. */
function ImageBlockView({ block }: { block: BlockOfType<'image'> }) {
  const [blob, setBlob] = useState<Blob | null>(null)
  const [url, setUrl] = useState<string | null>(null)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    let objectUrl: string | null = null
    let canceled = false
    fetchArtifactBlob(block.artifact_id)
      .then((loaded) => {
        if (canceled) return
        setBlob(loaded)
        if (!isPassiveImage(loaded)) return
        objectUrl = URL.createObjectURL(loaded)
        setUrl(objectUrl)
      })
      .catch(() => {
        if (!canceled) setFailed(true)
      })
    return () => {
      canceled = true
      if (objectUrl !== null) URL.revokeObjectURL(objectUrl)
    }
  }, [block.artifact_id])

  const alt = block.alt ?? 'attached image'
  return (
    <Card className="block-image" data-testid="image-block">
      {failed ? (
        <CardBody className="block-image-failed">Image unavailable</CardBody>
      ) : (
        <>
          {url !== null && <img src={url} alt={alt} />}
          {blob !== null && url === null && (
            <CardBody className="block-image-no-preview">
              No preview for this file type
            </CardBody>
          )}
          <CardFooter className="block-image-caption">
            <span>{alt}</span>
            {url !== null && (
              <a href={url} target="_blank" rel="noreferrer noopener">
                Open
              </a>
            )}
            {blob !== null && (
              <Button variant="link" onClick={() => saveBlob(blob, alt)}>
                Save
              </Button>
            )}
          </CardFooter>
        </>
      )}
    </Card>
  )
}

/** `184 KB`, `2.1 MB`: the size a reader expects on a file card. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

/** A non-image artifact: a kind tile, the name over its kind and size,
 *  and one Save that downloads through the daemon. */
function FileBlockView({ block }: { block: BlockOfType<'file'> }) {
  const meta = [
    block.mime ?? null,
    block.size_bytes != null ? formatBytes(block.size_bytes) : null,
  ].filter((part) => part !== null)
  const save = async () => {
    saveBlob(await fetchArtifactBlob(block.artifact_id), block.name)
  }
  return (
    <Card className="block-file" data-testid="file-block">
      <CardBody className="block-strip">
        <span className="block-file-tile" aria-hidden>
          <FileIcon size={16} focusable="false" />
        </span>
        <span className="block-file-text">
          <span className="block-file-name">{block.name}</span>
          {meta.length > 0 && (
            <span className="block-file-meta">{meta.join(' · ')}</span>
          )}
        </span>
        <span className="block-strip-open">
          <Button size="sm" onClick={() => void save()}>
            Save
          </Button>
        </span>
      </CardBody>
    </Card>
  )
}

/** The exhaustive switch over the generated union. */
function KnownBlockView({ block, api }: { block: KnownBlock; api: ApiClient }) {
  switch (block.type) {
    case 'markdown':
      return (
        <div className="block-markdown prose">
          <Prose>{block.text}</Prose>
        </div>
      )
    case 'approval_card':
      // A denormalized card whose row is missing is not a card.
      if (typeof block.request_id !== 'string') break
      return (
        <ApprovalCard
          api={api}
          requestId={block.request_id}
          title={block.title}
          body={block.body}
        />
      )
    case 'image':
      if (typeof block.artifact_id !== 'string') break
      return <ImageBlockView block={block} />
    case 'file':
      if (typeof block.artifact_id !== 'string') break
      return <FileBlockView block={block} />
    case 'progress':
      // The daemon's derived line: live over ephemeral frames
      // while the run works, the terminal text once it ends.
      return (
        <div
          className="block-progress"
          data-testid="progress-block"
          aria-live="polite"
        >
          <Clock size={16} aria-hidden focusable="false" />
          <Prose>{block.text}</Prose>
        </div>
      )
    case 'table':
      if (!Array.isArray(block.columns)) break
      return <TableBlock block={block} />
    case 'form':
      // A denormalized view whose row is missing is not a form.
      if (typeof block.request_id !== 'string') break
      return (
        <RequestForm
          api={api}
          requestId={block.request_id}
          title={block.title}
          fields={block.fields ?? []}
          submitLabel={block.submit_label}
        />
      )
    case 'choice_card':
      if (typeof block.request_id !== 'string') break
      return (
        <ChoiceCard
          api={api}
          requestId={block.request_id}
          title={block.title}
          body={block.body}
          options={block.options ?? []}
        />
      )
    case 'call':
      // The strip while the call runs, and the settled record after
      // (ADR-0022). Opening it opens the call inspector.
      if (typeof block.call_id !== 'string') break
      return <CallBlock callId={block.call_id} api={api} />
    case 'widget':
      // One Widget of a Software Package in a sandboxed frame
      // (ADR-0016). Without a tool call there is no view to render.
      if (typeof block.tool_call_id !== 'string') break
      return <WidgetBlock block={block} api={api} />
    case 'mail':
      // One line for an inbound mail that woke the Agent and for a
      // mail it sent (ADR-0019). Opening it opens the mail
      // inspector, which reads the words live.
      if (typeof block.message_id !== 'string') break
      return <MailBlock block={block} />
    case 'coding_session':
      // One card, running or settled (ADR-0033). It reads the session
      // record, and "Open" goes to the session page.
      if (typeof block.coding_session_id !== 'string') break
      return <CodingSessionBlock sessionId={block.coding_session_id} api={api} />
    case 'screen':
      // Typed here with no renderer: it takes the same fallback an
      // unknown type takes.
      break
    default: {
      // A new variant in the daemon's union fails this assignment, so
      // the switch cannot silently fall out of date.
      const unhandled: never = block
      return <UnknownBlock block={unhandled} />
    }
  }
  return <UnknownBlock block={block} />
}

function BlockView({ block, api }: { block: unknown; api: ApiClient }) {
  if (!isBlockShaped(block)) {
    return <UnknownBlock block={{ type: 'invalid' }} />
  }
  if (!isKnown(block)) {
    return <UnknownBlock block={block} />
  }
  return <KnownBlockView block={block} api={api} />
}

/** Render a message's block array; a non-array renders nothing. */
export function Blocks({
  blocks,
  api,
}: {
  blocks: readonly KnownBlock[]
  api: ApiClient
}) {
  if (!Array.isArray(blocks)) return null
  return (
    <>
      {blocks.map((block, index) => (
        <BlockView key={index} block={block} api={api} />
      ))}
    </>
  )
}
