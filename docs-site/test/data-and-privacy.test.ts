// What Pagis encrypts at rest, and what a model provider and the Push Relay
// receive: the "What Pagis encrypts" section of the Data and privacy page,
// and the links to it from the backup pages of the Headless Server and the
// Client App. An operator reads these sections before they keep a Backup or
// select a disk, so a change that deletes the section, drops a store from it
// or unlinks it fails here.

import { describe, expect, it } from 'vitest';
import { source } from '@/lib/source';
import { links, requiredSection, section } from './markdown';

const PRIVACY = ['data-and-privacy'];
const ENCRYPTION = 'What Pagis encrypts';
const MODEL_PROVIDER = 'What the model provider receives';
const PUSH_RELAY = 'What the Push Relay receives';
const ENCRYPTION_URL = '/data-and-privacy#what-pagis-encrypts';
const MODEL_PROVIDER_URL = '/data-and-privacy#what-the-model-provider-receives';

async function text(slugs: string[]): Promise<string> {
  const page = source.getPage(slugs);
  if (!page) throw new Error(`/${slugs.join('/')} is not a page`);
  return page.data.getText('raw');
}

/** The cells of each body row of every Markdown table in `text`. */
function tableRows(text: string): string[][] {
  const rows: string[][] = [];
  let header = true;
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (!line.startsWith('|')) {
      header = true;
      continue;
    }
    const cells = line
      .replace(/^\||\|$/g, '')
      .split('|')
      .map((cell) => cell.trim());
    const delimiter = cells.every((cell) => /^[-:]+$/.test(cell));
    if (header) header = false;
    else if (!delimiter) rows.push(cells);
  }
  return rows;
}

/** Every `code` span in `text`, without the backticks. */
function codeSpans(text: string): string[] {
  return text.split('`').filter((_, index) => index % 2 === 1);
}

async function encryptionRows(): Promise<string[][]> {
  const rows = tableRows(requiredSection(await text(PRIVACY), 2, ENCRYPTION));
  expect(rows, `the "${ENCRYPTION}" section has no table of stores`).not.toEqual([]);
  return rows;
}

/** The row whose first cell `found` accepts says whether Pagis encrypts the store. */
function expectAnsweredRow(rows: string[][], store: string, found: (cell: string) => boolean) {
  const row = rows.find((cells) => found(cells[0] ?? ''));
  expect(row, `the "${ENCRYPTION}" table has no row for ${store}`).toBeDefined();
  expect(row?.[2] ?? '', `the row of ${store} does not say whether Pagis encrypts it`).toMatch(
    /^(Yes|No|Partly)/,
  );
}

describe('the Data and privacy page', () => {
  it('has a "What Pagis encrypts" section', async () => {
    requiredSection(await text(PRIVACY), 2, ENCRYPTION);
  });

  it('names each path of the State Directory in the encryption table', async () => {
    const data = requiredSection(await text(PRIVACY), 2, 'Data and configuration');
    const paths = tableRows(data).flatMap((row) => codeSpans(row[0] ?? ''));
    expect(paths).toContain('secrets.enc');
    expect(paths).toContain('pagis.db');

    const rows = await encryptionRows();
    for (const path of paths) {
      expectAnsweredRow(rows, `\`${path}\``, (cell) => codeSpans(cell).includes(path));
    }
  });

  it('names the database and the Computer volumes in the encryption table', async () => {
    const rows = await encryptionRows();
    expectAnsweredRow(rows, 'the Postgres database', (cell) => cell.includes('Postgres'));
    expectAnsweredRow(rows, 'the Computer volumes', (cell) => cell.includes('Computer volume'));
  });

  it('says what the model provider receives', async () => {
    const encryption = requiredSection(await text(PRIVACY), 2, ENCRYPTION);
    expect(section(encryption, 3, MODEL_PROVIDER)).toBeDefined();
  });

  it('says what the Push Relay receives, after what the model provider receives', async () => {
    const encryption = requiredSection(await text(PRIVACY), 2, ENCRYPTION);
    const relay = section(encryption, 3, PUSH_RELAY);
    expect(relay).toBeDefined();
    expect(encryption.indexOf(`### ${PUSH_RELAY}`)).toBeGreaterThan(
      encryption.indexOf(`### ${MODEL_PROVIDER}`),
    );
    expect(relay).toContain('cannot read a Notification');
  });

  it('links the part on several People to what the model provider receives', async () => {
    const several = requiredSection(await text(PRIVACY), 2, 'Several People on a local installation');
    expect(links(several)).toContain('#what-the-model-provider-receives');
  });
});

describe('the pages that link to the Data and privacy page', () => {
  it('link the backup of a Headless Server to what Pagis encrypts', async () => {
    expect(links(await text(['server', 'backup']))).toContain(ENCRYPTION_URL);
  });

  it('link the backup of the Client App to what Pagis encrypts', async () => {
    expect(links(await text(['client-app', 'backup']))).toContain(ENCRYPTION_URL);
  });

  it('link the provider keys of the first Administrator to what the model provider receives', async () => {
    const administrator = requiredSection(
      await text(['server', 'administration']),
      2,
      'The first Administrator',
    );
    expect(links(administrator)).toContain(MODEL_PROVIDER_URL);
  });
});

describe('a section of a page', () => {
  it('does not end at a hash line in a code block', () => {
    const document = '## One\n\ntext\n\n```bash\n# a comment\n```\n\nmore\n\n## Two\n\nother\n';
    const one = section(document, 2, 'One');
    expect(one).toContain('more');
    expect(one).not.toContain('other');
  });
});
