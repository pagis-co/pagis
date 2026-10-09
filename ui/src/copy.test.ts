import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import ts from 'typescript'
import { describe, expect, it } from 'vitest'

// The product calls an Agent a sprite in the words it shows the Person
// (CONTEXT.md, Agent). The code keeps the word agent. This test reads
// every source file of the Product App and the Administration Interface
// and fails on copy that says agent: JSX text, and each string or
// template text that holds a space or starts with a capital letter. A
// lower-case string without a space is an identifier, a key, a path or
// a class name (`'agent'` as a kind), and is not copy.

const srcDir = dirname(fileURLToPath(import.meta.url))

// The word agent alone. A hyphen, an underscore, a slash, a colon or a
// dot next to it makes it part of an identifier (`agent-desk`,
// `agent_id`, `agent:…`), not a word of copy.
const agentWord = /(?<![\w\-/.:])agents?(?![\w\-/_])/i

// Copy that says agent for something that is not an Agent. Each entry
// is `file: text` and gives its reason.
const allowed = new Set<string>([
  // The welcome step says what a sprite is. "AI agents" names the
  // general kind of software, not an Agent.
  'components/onboarding/WelcomeStep.tsx: Sprites are AI agents that work as your virtual assistants. They use their own computers, remember useful context, and follow up on tasks.',
])

function sourcePaths(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) return entry.name === 'test' ? [] : sourcePaths(path)
    if (!/\.tsx?$/.test(entry.name)) return []
    if (/\.test\.tsx?$/.test(entry.name) || entry.name.endsWith('.d.ts')) return []
    return [path]
  })
}

/** Each piece of text in the source `text` of `file` that can reach
 * the screen. */
function copyIn(file: string, text: string): string[] {
  const source = ts.createSourceFile(
    file,
    text,
    ts.ScriptTarget.Latest,
    true,
    file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  )
  const found: string[] = []
  const visit = (node: ts.Node): void => {
    if (ts.isJsxText(node)) {
      found.push(node.text)
    } else if (
      ts.isStringLiteral(node) ||
      ts.isNoSubstitutionTemplateLiteral(node) ||
      ts.isTemplateHead(node) ||
      ts.isTemplateMiddle(node) ||
      ts.isTemplateTail(node)
    ) {
      if (/\s|^[A-Z]/.test(node.text)) found.push(node.text)
    }
    ts.forEachChild(node, visit)
  }
  visit(source)
  return found.map((text) => text.replace(/\s+/g, ' ').trim()).filter((text) => text !== '')
}

describe('Person-facing copy', () => {
  it('says sprite for an Agent in the Product App and the Administration Interface', () => {
    const offending = sourcePaths(srcDir).flatMap((path) => {
      const file = relative(srcDir, path)
      return copyIn(path, readFileSync(path, 'utf8'))
        .filter((text) => agentWord.test(text))
        .map((text) => `${file}: ${text}`)
        .filter((entry) => !allowed.has(entry))
    })
    expect(offending).toEqual([])
  })

  it('finds agent in JSX text, attributes and template text, and not in identifiers', () => {
    // The scan must see the copy it guards: a scan that finds nothing
    // passes on any source.
    const says = (code: string) =>
      copyIn('probe.tsx', `const x = ${code}`).some((text) => agentWord.test(text))
    expect(says('<p>An empty desk · create an agent</p>')).toBe(true)
    expect(says('<b aria-label="Every Agent of the installation" />')).toBe(true)
    expect(says('`nothing bounds what an agent keeps ${y}`')).toBe(true)
    expect(says("toast('Agents know it')")).toBe(true)
    expect(says("name ?? 'Agent'")).toBe(true)
    expect(says("kind === 'agent'")).toBe(false)
    expect(says("'agent_hangup'")).toBe(false)
    expect(says("'agent-memory-meta agent-memory-files'")).toBe(false)
    expect(says('`/api/v1/agents/${id}`')).toBe(false)
    expect(says('<p>{agent.name} desk</p>')).toBe(false)
  })
})
