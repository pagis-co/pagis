import { existsSync, readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { scanURLs, validateFiles } from 'next-validate-link';
import { describe, expect, it } from 'vitest';
import { source } from '@/lib/source';

const pages = source.getPages();

describe('the content', () => {
  it('serves the introduction at the root of the site', () => {
    expect(source.getPage([])?.url).toBe('/');
  });

  it('gives each page a title and a description', () => {
    const incomplete = pages
      .filter((page) => !page.data.title || !page.data.description)
      .map((page) => page.path);
    expect(incomplete).toEqual([]);
  });

  it('links only to pages, headings and files that exist', async () => {
    const scanned = await scanURLs({
      preset: 'next',
      populate: {
        '[[...slug]]': pages.map((page) => ({
          value: { slug: page.slugs },
          hashes: page.data.toc.map((item) => item.url.slice(1)),
        })),
      },
    });
    const files = await Promise.all(
      pages.map(async (page) => ({
        path: page.absolutePath ?? page.path,
        content: await page.data.getText('raw'),
        url: page.url,
      })),
    );
    const results = await validateFiles(files, {
      scanned,
      markdown: {
        components: {
          Card: { attributes: ['href'] },
          Video: { attributes: ['src', 'poster'] },
        },
      },
      checkRelativePaths: 'as-url',
      whitelist: (url) => publicFileExists(url),
    });
    const broken = results.flatMap((result) =>
      result.errors.map((error) => `${result.file}:${error.line}: ${error.url}`),
    );
    expect(broken).toEqual([]);
  });

  // The link check above leaves a link to a heading of the same page out.
  it('links only to headings of the same page that exist', async () => {
    const broken: string[] = [];
    for (const page of pages) {
      const headings = new Set(page.data.toc.map((item) => item.url));
      const content = await page.data.getText('raw');
      for (const [, fragment] of content.matchAll(/\]\((#[^)\s]+)\)/g)) {
        if (!headings.has(fragment)) broken.push(`${page.path}: ${fragment}`);
      }
    }
    expect(broken).toEqual([]);
  });

  // The product and its code comments send people to pages of this site,
  // so a page that moves or a heading that changes must not break them.
  it('has each page and heading that the product links to', () => {
    const urls = new Map(
      pages.map((page) => [page.url, new Set(page.data.toc.map((item) => item.url.slice(1)))]),
    );
    const broken = productLinks().filter(({ url }) => {
      const [pathname, hash] = url.split('#');
      const headings = urls.get(pathname.replace(/\/$/, '') || '/');
      return !headings || (hash !== undefined && !headings.has(hash));
    });
    expect(broken).toEqual([]);
  });
});

const repository = path.join(import.meta.dirname, '..', '..');

/** Each link to this site in the code of the product and its documents. */
function productLinks(): { file: string; url: string }[] {
  const roots = ['ui/src', 'desktop/src', 'desktop/static', 'crates', 'deploy', 'docs', 'xtask/src'];
  const files = roots.flatMap((root) => walk(path.join(repository, root)));
  files.push(...['README.md', 'CONTRIBUTING.md', 'Dockerfile'].map((file) => path.join(repository, file)));
  return files.flatMap((file) =>
    [...readFileSync(file, 'utf8').matchAll(/docs\.pagis\.co(\/[\w\-/#]*)?/g)].map((match) => ({
      file: path.relative(repository, file),
      url: (match[1] ?? '/').replace(/[.)]+$/, '') || '/',
    })),
  );
}

function walk(directory: string): string[] {
  if (!existsSync(directory)) return [];
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) return ['node_modules', 'target', 'dist'].includes(entry.name) ? [] : walk(full);
    return /\.(rs|ts|tsx|html|md|sh|yaml|toml)$/.test(entry.name) || entry.name === 'Caddyfile' ? [full] : [];
  });
}

/** A root-relative URL that names a file in `public/`, such as a video. */
function publicFileExists(url: string): boolean {
  if (!url.startsWith('/') || url.startsWith('//')) return false;
  const file = path.join(import.meta.dirname, '..', 'public', decodeURIComponent(url));
  return path.extname(file) !== '' && existsSync(file);
}
