# 0026: The documentation site builds from the tree of a release

Status: accepted.

## Context

The people who use Pagis need a documentation site with search, guides,
screenshots and videos, at docs.pagis.co. The pages change with the code, so
they belong in the same repository and in the same pull request as the change
that they describe. A person installs a release, not `main`, so the site must
describe the release that is published.

The documentation of established developer products takes one of four forms:

- **A hosted platform** (Mintlify). It reads MDX from the repository and
  serves the site. It has the most finished look for the least work. The site
  runs on the platform and not on a host of our choice, the plan of the
  platform sets the features, and the components are those of the platform.
- **A documentation framework on React** (Fumadocs on Next.js, Docusaurus).
  The site is an application in the repository. Fumadocs gives the look of
  the hosted platforms and is a Next.js application. Docusaurus keeps a copy
  of the pages for each version in the repository, and its default look is
  plainer.
- **A documentation framework on another stack** (Starlight on Astro,
  VitePress on Vue). The output is static, and the look is finished. The
  repository then holds a second UI framework beside the React of `ui/`.
- **A site of its own** (Stripe, on Markdoc). It gives full control, at the
  cost of a team that builds the site.

The site is on Cloudflare. A Next.js site runs on Cloudflare in two forms:

- **A static export** served by a Worker of static assets alone. Cloudflare
  serves each file, and a request for a static asset costs nothing on any
  plan. The site then has no code at request time: the search index is a
  file, an image is not resized, and a response cannot depend on a request
  header.
- **The full Next.js server** in a Worker, through the OpenNext adapter. The
  site keeps each Next.js feature, and each request runs the Worker. The
  adapter is a layer between Next.js and Cloudflare to keep current, and a
  Next.js Worker is often larger than the free plan allows.

Every page of the site is known at build time, so the site needs no code at
request time.

## Decision

- **Fumadocs on Next.js, in `docs-site/`.** Each page is an MDX file in
  `docs-site/content/`. The site is React and TypeScript with Vitest, as `ui/`
  and `desktop/` are, and it takes its colors and its mark from the product
  (`ui/src/tokens.css`, `assets/brand`). It gives search, a Markdown copy of
  each page for agents, `llms.txt` and an Open Graph image for each page with
  no other service.
- **A static export on Cloudflare.** `next build` writes each page, the
  search index, the Markdown copies and the Open Graph images to `out/`.
  `docs-site/wrangler.jsonc` makes it a Worker of static assets alone, with
  docs.pagis.co as its custom domain. `public/_redirects` serves the Markdown
  copy of a page at its address with `.md` added. The site runs no code of
  its own, so it can move to any static host.
- **Production is the tree of the latest release tag.** A `v*` tag builds the
  site from its own tree and deploys it to docs.pagis.co with Wrangler
  (`.github/workflows/docs.yml`). A pull request uploads a preview version
  that does not change production, and a push to `main` deploys nothing. The navigation bar shows the workspace
  version of the tree, so the version on the site and the pages agree.
- **The gate checks the site.** `cargo xtask` runs the type check, the tests,
  the build and the test of the export for a change in `docs-site/`, and CI
  runs them for each pull request. The tests fail on a page with no title or
  description, and on a link to a page, a heading or a file that does not
  exist. The build fails on a page that does not compile, on a missing image
  and on a part that needs a server. The test of the export serves `out/` in
  the local runtime of Cloudflare and reads each address.
- **Media stay small in git.** A screenshot and a short silent clip go in
  `docs-site/public/media/`. A longer video goes on a video host.

## Consequences

- docs.pagis.co shows one release. A change to a page shows there with the
  next release, or when the workflow deploys a tag again. The site does not
  serve the pages of an older release.
- The site holds every guide for the people who use Pagis: the Client App,
  the Headless Server, the Plugins and Widgets, and data and privacy. The
  documents in `docs/` are for the people who build Pagis, and no guide is in
  both places. The product and the code comments link to pages of the site,
  and a test of the site fails on such a link to a page or a heading that does
  not exist.
- The site sends no Markdown for a request that asks for it in the `Accept`
  header, and it does not resize a screenshot for each screen. Either needs
  code at request time.
- The Cloudflare account, its `pagis.co` zone and the two secrets of the
  workflow are configuration outside the repository. `docs-site/README.md`
  names them.
