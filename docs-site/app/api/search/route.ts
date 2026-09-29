import { createFromSource } from 'fumadocs-core/search/server';
import { source } from '@/lib/source';

export const revalidate = false;

// The build writes the search index as one file, and the search dialog
// searches it in the browser.
export const { staticGET: GET } = createFromSource(source, { language: 'english' });
