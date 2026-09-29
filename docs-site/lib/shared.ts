import { createGetUrl } from 'fumadocs-core/source';

export const siteName = 'Pagis';
export const siteUrl = 'https://docs.pagis.co';
export const docsRoute = '/';
export const docsImageRoute = '/og';
export const docsContentRoute = '/llms.mdx';

export const gitConfig = {
  user: 'pagis-co',
  repo: 'pagis',
  branch: 'main',
  contentDir: 'docs-site/content',
};

const getContentUrl = createGetUrl(docsContentRoute);

/** The URL of the Markdown copy of a page, for agents and "Copy page". */
export function getPageMarkdownUrl(page: { slugs: string[]; locale?: string }) {
  const segments = [...page.slugs, 'content.md'];

  return { segments, url: getContentUrl(segments, page.locale) };
}

const getImageUrl = createGetUrl(docsImageRoute);

/** The URL of the Open Graph image of a page. */
export function getPageImageUrl(page: { slugs: string[]; locale?: string }) {
  const segments = [...page.slugs, 'image.png'];

  return { segments, url: getImageUrl(segments, page.locale) };
}
