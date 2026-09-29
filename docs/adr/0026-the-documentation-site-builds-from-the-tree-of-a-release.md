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
  runs on the platform and not on Vercel, the plan of the platform sets the
  features, and the components are those of the platform.
- **A documentation framework on React** (Fumadocs on Next.js, Docusaurus).
  The site is an application in the repository. Fumadocs gives the look of
  the hosted platforms and is a Next.js application, which Vercel builds with
  no configuration. Docusaurus keeps a copy of the pages for each version in
  the repository, and its default look is plainer.
- **A documentation framework on another stack** (Starlight on Astro,
  VitePress on Vue). The output is static, and the look is finished. The
  repository then holds a second UI framework beside the React of `ui/`.
- **A site of its own** (Stripe, on Markdoc). It gives full control, at the
  cost of a team that builds the site.

## Decision

- **Fumadocs on Next.js, in `docs-site/`.** Each page is an MDX file in
  `docs-site/content/`. The site is React and TypeScript with Vitest, as `ui/`
  and `desktop/` are, and it takes its colors and its mark from the product
  (`ui/src/tokens.css`, `assets/brand`). It gives search, a Markdown copy of
  each page for agents, `llms.txt` and an Open Graph image for each page with
  no other service.
- **Production is the tree of the latest release tag.** A `v*` tag builds the
  site from its own tree and deploys it to docs.pagis.co through the Vercel
  CLI (`.github/workflows/docs.yml`). A pull request gets a preview, and a
  push to `main` deploys nothing. The navigation bar shows the workspace
  version of the tree, so the version on the site and the pages agree.
- **The gate checks the site.** `cargo xtask` runs the type check, the tests
  and the build of the site for a change in `docs-site/`, and CI runs them for
  each pull request. The tests fail on a page with no title or description,
  and on a link to a page, a heading or a file that does not exist. The build
  fails on a page that does not compile and on a missing image.
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
- The Vercel project and the secrets of the `docs` environment are
  configuration outside the repository. `docs-site/README.md` names them.
