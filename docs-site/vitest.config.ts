import { fileURLToPath } from 'node:url';
import mdx from 'fumadocs-mdx/vite';
import { defineConfig } from 'vitest/config';

// The Fumadocs MDX plugin compiles the content, so a test reads the same
// page tree as the site. The `unit` project checks the sources, and the
// `export` project checks the export of `npm run build` as Cloudflare
// serves it.
export default defineConfig({
  plugins: [mdx()],
  resolve: {
    alias: { '@': fileURLToPath(new URL('.', import.meta.url)) },
  },
  test: {
    projects: [
      {
        extends: true,
        test: { name: 'unit', include: ['test/*.test.{ts,tsx}'] },
      },
      {
        extends: true,
        test: { name: 'export', include: ['test/export/*.test.ts'], testTimeout: 30_000 },
      },
    ],
  },
});
