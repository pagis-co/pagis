import { readFileSync } from 'node:fs';
import path from 'node:path';

/**
 * The release that the documentation describes: the workspace version in
 * the `Cargo.toml` of the same tree. A release tag builds the site from
 * its own tree, so the pages and the version agree.
 */
export function release(): string {
  const manifest = path.join(process.cwd(), '..', 'Cargo.toml');
  return workspaceVersion(readFileSync(manifest, 'utf8'));
}

/** The `version` of the `[workspace.package]` table of a Cargo manifest. */
export function workspaceVersion(manifest: string): string {
  let inTable = false;
  for (const line of manifest.split('\n')) {
    const header = line.match(/^\s*\[([^\]]+)\]/);
    if (header) {
      inTable = header[1].trim() === 'workspace.package';
      continue;
    }
    const version = inTable && line.match(/^\s*version\s*=\s*"([^"]+)"/);
    if (version) return version[1];
  }
  throw new Error('The Cargo manifest has no [workspace.package] version');
}
