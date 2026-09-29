import { createMDX } from 'fumadocs-mdx/next';

const withMDX = createMDX();

/**
 * The site is a static export: `next build` writes each page, image and
 * Markdown copy to `out/`, and Cloudflare serves the files.
 *
 * @type {import('next').NextConfig}
 */
const config = {
  output: 'export',
  reactStrictMode: true,
  // A static export has no server to resize an image, so the build copies
  // each image as it is.
  images: { unoptimized: true },
};

export default withMDX(config);
