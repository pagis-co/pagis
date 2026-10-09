// The Coding Sessions page: how a Person signs in to a Coding Harness, who
// answers a Harness Permission, and the terms of a harness subscription. A
// Person reads these parts before they let an Agent run a harness on their
// computer, so a change that drops a part, renames a UI label or removes
// the statement about the credential fails here.

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { source } from '@/lib/source';
import { links, requiredSection } from './markdown';

const PAGE = ['coding-sessions'];
const SIGN_IN = 'Sign in to a harness';
const WHO_ANSWERS = 'Choose who answers a harness';
const TERMS = 'The terms of a harness subscription';
const ANTHROPIC_TERMS = 'https://code.claude.com/docs/en/legal-and-compliance';

async function text(slugs: string[]): Promise<string> {
  const page = source.getPage(slugs);
  if (!page) throw new Error(`/${slugs.join('/')} is not a page`);
  return page.data.getText('raw');
}

/** The pages of the "Get started" part of the sidebar, in order. */
function getStarted(): string[] {
  const meta = JSON.parse(
    readFileSync(path.join(import.meta.dirname, '..', 'content', 'meta.json'), 'utf8'),
  ) as { pages: string[] };
  const start = meta.pages.indexOf('---Get started---');
  const rest = meta.pages.slice(start + 1);
  const end = rest.findIndex((entry) => entry.startsWith('---'));
  return end === -1 ? rest : rest.slice(0, end);
}

describe('the Coding Sessions page', () => {
  it('is a page under "Get started"', () => {
    expect(source.getPage(PAGE)?.url).toBe('/coding-sessions');
    expect(getStarted()).toContain('coding-sessions');
  });

  it('has the parts on sign-in, on who answers and on the terms', async () => {
    const page = await text(PAGE);
    for (const name of [SIGN_IN, WHO_ANSWERS, TERMS]) requiredSection(page, 2, name);
  });

  it('names the approval modes and the switch with the words of the Product App', async () => {
    const part = requiredSection(await text(PAGE), 2, WHO_ANSWERS);
    expect(part).toContain('Ask me');
    expect(part).toContain('Let the sprite decide');
    expect(part).toContain('Allow modes that act without asking');
  });

  it('names the sign-in buttons of Settings', async () => {
    const part = requiredSection(await text(PAGE), 2, SIGN_IN);
    expect(part).toContain('Sign in with a subscription');
    expect(part).toContain('Sign in with an API key');
  });

  it('links the terms of Anthropic and says that Pagis never holds the credential', async () => {
    const part = requiredSection(await text(PAGE), 2, TERMS);
    expect(links(part).some((link) => link.startsWith(ANTHROPIC_TERMS))).toBe(true);
    expect(part).toMatch(/never reads, copies, stores or relays/);
  });
});

describe('the Data and privacy page', () => {
  it('says what a Coding Harness sends', async () => {
    requiredSection(await text(['data-and-privacy']), 3, 'What a Coding Harness sends');
  });
});

describe('the introduction', () => {
  it('links what an Agent does to the Coding Sessions page', async () => {
    const part = requiredSection(await text([]), 2, 'What an Agent does');
    expect(links(part)).toContain('/coding-sessions');
  });
});
