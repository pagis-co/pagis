// The token values of `tokens.css` in each theme, for the tests that
// hold a page or a stylesheet to them.

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const tokensPath = join(dirname(fileURLToPath(import.meta.url)), '..', 'tokens.css')

export function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, '')
}

/** The declarations of one rule of `tokens.css`, by token name. */
function ruleTokens(css: string, selector: string): Record<string, string> {
  const start = css.indexOf(selector)
  if (start < 0) throw new Error(`tokens.css has no rule ${selector}`)
  const open = css.indexOf('{', start)
  let depth = 0
  let end = open
  for (; end < css.length; end += 1) {
    if (css[end] === '{') depth += 1
    else if (css[end] === '}' && (depth -= 1) === 0) break
  }
  const body = css.slice(open + 1, end)
  return Object.fromEntries(
    [...body.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]),
  )
}

/** The two themes. The dark rule redefines the primitive tokens only,
 * so it reads on top of the light rule, exactly as the browser does. */
export function themes(): { name: 'light' | 'dark'; tokens: Record<string, string> }[] {
  const css = stripComments(readFileSync(tokensPath, 'utf8'))
  const light = ruleTokens(css, ':root {')
  return [
    { name: 'light', tokens: light },
    { name: 'dark', tokens: { ...light, ...ruleTokens(css, ":root[data-theme='dark']") } },
  ]
}

/** The value of a token, with each alias to another token resolved. */
export function value(tokens: Record<string, string>, name: string): string {
  let color = tokens[name]
  if (color === undefined) throw new Error(`tokens.css has no ${name}`)
  for (let alias = /^var\((--[\w-]+)\)$/.exec(color); alias !== null; ) {
    color = tokens[alias[1]]
    if (color === undefined) throw new Error(`tokens.css has no ${alias[1]}`)
    alias = /^var\((--[\w-]+)\)$/.exec(color)
  }
  return color
}
