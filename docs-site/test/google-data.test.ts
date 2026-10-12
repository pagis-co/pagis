import { describe, expect, it } from 'vitest';
import { source } from '@/lib/source';
import { links, requiredSection } from './markdown';

async function text(slugs: string[]): Promise<string> {
  const page = source.getPage(slugs);
  if (!page) throw new Error(`/${slugs.join('/')} is not a page`);
  return page.data.getText('raw');
}

describe('the Google data guide', () => {
  it('appears in the navigation with its deletion title', () => {
    const page = source.getPage(['google-data']);
    expect(page?.data.title).toBe('Delete the Google data that Pagis holds');
    expect(JSON.stringify(source.pageTree)).toContain('"url":"/google-data"');
  });

  it('explains each deletion action and the data that stays', async () => {
    const content = await text(['google-data']);
    expect(requiredSection(content, 2, 'Disconnect the Connection')).toContain('refresh token');
    expect(requiredSection(content, 2, 'Forget a source')).toContain('not built');
    expect(links(requiredSection(content, 2, 'Remove access at Google'))).toContain('https://myaccount.google.com/permissions');
    expect(requiredSection(content, 2, 'When a Person leaves a Server')).toContain('not built');
    expect(requiredSection(content, 2, 'What stays in a Backup')).toContain('no time limit');
  });

  it('links from privacy and from the verification instructions', async () => {
    expect(links(await text(['data-and-privacy']))).toContain('/google-data');
    const setup = requiredSection(await text(['server', 'administration']), 3, 'The Google OAuth client');
    expect(links(setup)).toContain('/google-data');
    expect(setup).toContain('disclosure');
    expect(setup).toContain('verification');
  });
});
