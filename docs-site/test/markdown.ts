// Helpers that read the Markdown source of a page in a test.

/**
 * The lines under the heading `name` of `level`, up to the next heading of
 * the same level or a higher one. A `#` line in a fenced code block is not
 * a heading.
 */
export function section(document: string, level: number, name: string): string | undefined {
  const lines: string[] = [];
  let inside = false;
  let fenced = false;
  for (const line of document.split('\n')) {
    if (line.trimStart().startsWith('```')) fenced = !fenced;
    const heading = fenced ? undefined : line.match(/^(#+) /)?.[1].length;
    if (inside) {
      if (heading !== undefined && heading <= level) break;
      lines.push(line);
    } else if (heading === level && line.trimEnd() === `${'#'.repeat(level)} ${name}`) {
      inside = true;
    }
  }
  return inside ? lines.join('\n') : undefined;
}

export function requiredSection(document: string, level: number, name: string): string {
  const found = section(document, level, name);
  if (found === undefined) throw new Error(`no "${'#'.repeat(level)} ${name}" section`);
  return found;
}

/** The target of each Markdown link in `text`. */
export function links(text: string): string[] {
  return [...text.matchAll(/\]\(([^)\s]+)\)/g)].map((match) => match[1]);
}
