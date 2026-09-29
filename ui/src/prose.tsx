// The prose renderer. One component draws every Markdown
// surface — a message block, a progress line, a memory page — so what
// an agent writes reads the same everywhere.
//
// GitHub Flavored Markdown is on. `add_block` offers a table block
// (ADR-0004), but an agent also writes a pipe table in its prose; under
// CommonMark alone that table falls through as one paragraph of pipe
// characters. The extension also carries strikethrough, task lists and
// bare links, which agents write for the same reason.
//
// Raw HTML stays off, which is react-markdown's own default: prose
// carries model text and text that reached the agent from outside.
//
// Prose loads no image. The author of the text chooses the address of
// an image, and the browser of each reader would send a request to that
// host, with data in the address. So an image shows as a link, as in a
// mail client that blocks remote images. An image that must show is an
// `image` block (ADR-0004).

import type { Element, Root, Text } from 'hast'
import type { ReactNode } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

import './prose.css'

/** A Markdown table in its own scroll box, so a wide one never makes
 *  the column that holds the message scroll sideways. */
function ProseTable({ children }: { children?: ReactNode }) {
  return (
    <div className="prose-table">
      <table>{children}</table>
    </div>
  )
}

/** Change each image into a link to its address, with the alt text as
 *  the link text. A link cannot hold a second link, so an image in a
 *  link changes into its alt text only. react-markdown checks the link
 *  target after this change, as it checks each other link. */
function imagesAsLinks() {
  return (tree: Root) => replaceImages(tree, false)
}

function replaceImages(parent: Root | Element, inLink: boolean) {
  parent.children.forEach((child, index) => {
    if (child.type !== 'element') return
    if (child.tagName !== 'img') {
      replaceImages(child, inLink || child.tagName === 'a')
      return
    }
    const alt = String(child.properties.alt ?? '').trim()
    const text: Text = { type: 'text', value: alt === '' ? 'image' : alt }
    parent.children[index] = inLink
      ? text
      : {
          type: 'element',
          tagName: 'a',
          properties: { href: child.properties.src },
          children: [text],
        }
  })
}

export function Prose({ children }: { children: string }) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      rehypePlugins={[imagesAsLinks]}
      components={{ table: ProseTable }}
    >
      {children}
    </ReactMarkdown>
  )
}
