// The rules of a stylesheet, for the tests that hold a stylesheet to
// the design system.

/** The bodies of every block that opens with `prelude`, nested blocks
 * included. */
export function blocks(css: string, prelude: string): string {
  const escaped = prelude.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const opening = new RegExp(`(?:^|\\n)\\s*${escaped}\\s*\\{`, 'g')
  const bodies = [...css.matchAll(opening)].map((match) => {
    const open = match.index + match[0].length - 1
    let depth = 0
    let end = open
    for (; end < css.length; end += 1) {
      if (css[end] === '{') depth += 1
      else if (css[end] === '}' && (depth -= 1) === 0) break
    }
    return css.slice(open + 1, end)
  })
  if (bodies.length === 0) throw new Error(`no block opens with ${prelude}`)
  return bodies.join('\n')
}
