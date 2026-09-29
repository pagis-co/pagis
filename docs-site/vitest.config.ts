import { fileURLToPath } from 'node:url';
import mdx from 'fumadocs-mdx/vite';
import { defineConfig } from 'vitest/config';

// The Fumadocs MDX plugin compiles the content, so a test reads the same
// page tree as the site.
export default defineConfig({
  plugins: [mdx()],
  resolve: {
    alias: { '@': fileURLToPath(new URL('.', import.meta.url)) },
  },
});
