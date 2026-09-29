// The exported site as Cloudflare serves it. The test starts the Worker of
// `wrangler.jsonc` in the local runtime of Cloudflare, over the files that
// `npm run build` wrote to `out/`, and reads each address as a browser or
// an agent does.

import { existsSync } from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { unstable_startWorker } from 'wrangler';
import { source } from '@/lib/source';

const site = path.join(import.meta.dirname, '..', '..');
const pages = source.getPages();

let worker: Awaited<ReturnType<typeof unstable_startWorker>>;
let base: URL;

beforeAll(async () => {
  if (!existsSync(path.join(site, 'out', 'index.html'))) {
    throw new Error('out/ holds no export. Run `npm run build` first.');
  }
  worker = await unstable_startWorker({
    config: path.join(site, 'wrangler.jsonc'),
    dev: { server: { port: 0 }, inspector: false, remote: false },
  });
  base = await worker.url;
}, 60_000);

afterAll(async () => {
  await worker?.dispose();
});

function get(address: string, headers: Record<string, string> = {}): Promise<Response> {
  return fetch(new URL(address, base), { redirect: 'manual', headers });
}

describe('the site on Cloudflare', () => {
  it('serves each page as HTML at its address', async () => {
    for (const page of pages) {
      const response = await get(page.url);
      expect(response.status, page.url).toBe(200);
      expect(response.headers.get('content-type'), page.url).toMatch(/^text\/html/);
      expect(await response.text(), page.url).toContain(`<title>${page.data.title}`);
    }
  });

  it('serves the Markdown copy of each page at its address with `.md` added', async () => {
    for (const page of pages) {
      const address = page.url === '/' ? '/index.md' : `${page.url}.md`;
      const response = await get(address);
      expect(response.status, address).toBe(200);
      expect(response.headers.get('content-type'), address).toMatch(/^text\/markdown/);
      expect(await response.text(), address).toMatch(new RegExp(`^# ${page.data.title} \\(`));
    }
  });

  it('sends an address with a trailing slash to the page', async () => {
    const response = await get('/quickstart/');
    expect(response.status).toBe(307);
    expect(response.headers.get('location')).toBe('/quickstart');
  });

  it('answers an unknown address with the 404 page', async () => {
    const response = await get('/no-such-page', { 'Sec-Fetch-Mode': 'navigate' });
    expect(response.status).toBe(404);
    expect(response.headers.get('content-type')).toMatch(/^text\/html/);
  });

  it('serves the search index as JSON', async () => {
    const response = await get('/api/search');
    expect(response.status).toBe(200);
    expect(response.headers.get('content-type')).toMatch(/^application\/json/);
    expect(await response.json()).toHaveProperty('type');
  });

  it('serves the Open Graph image of each page', async () => {
    for (const page of pages) {
      const address = `/og/${[...page.slugs, 'image.png'].join('/')}`;
      const response = await get(address);
      expect(response.status, address).toBe(200);
      expect(response.headers.get('content-type'), address).toBe('image/png');
    }
  });

  it('serves the index of the pages for agents', async () => {
    const response = await get('/llms.txt');
    expect(response.status).toBe(200);
    expect(await response.text()).toContain('[Quickstart](/quickstart)');
  });
});
