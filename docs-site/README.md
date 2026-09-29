# The documentation site

`docs-site/` is the source of the user documentation at
[docs.pagis.co](https://docs.pagis.co). It is a [Fumadocs](https://fumadocs.dev)
site on Next.js, and each page is an MDX file in `content/`. The build is a
static export to `out/`, and Cloudflare serves the files.

The documents in `docs/` are for the people who build Pagis: the
architecture, the decisions and the release procedures. The pages in
`docs-site/` are for the people who use Pagis.

## Run the site

```bash
cd docs-site
npm ci
npm run dev      # http://localhost:3000, with each change to a page live
```

To see the export as Cloudflare serves it, build it and serve `out/` in the
local runtime of Cloudflare:

```bash
npm run build
npm run preview  # http://localhost:8787
```

The navigation bar shows the release that the pages describe: the
workspace version in the `Cargo.toml` at the root of the repository.

## Write a page

A page is an MDX file in `content/`. Its path gives its URL:
`content/quickstart.mdx` is `/quickstart`, and `content/index.mdx` is the
root of the site.

```mdx
---
title: Quickstart
description: Install the Client App on macOS or Linux and start your first Agent.
icon: Rocket
---

The Client App runs Pagis on your own computer.
```

- `title` and `description` are necessary. The search, the link previews
  and `llms.txt` show them.
- `icon` is optional. It is the name of a [Lucide](https://lucide.dev/icons)
  icon, which the sidebar shows.
- `content/meta.json` gives the order of the pages in the sidebar. A
  `---Name---` entry starts a section. A folder with its own `meta.json`
  is a group of pages.
- Write in ASD-STE100 Simplified Technical English, in the present tense,
  and use the terms of [CONTEXT.md](../CONTEXT.md). A page states the
  release as it is.

To link to another page, give its file path, for example
`[Headless Server](./server/index.mdx)`, or its URL, `/server`.
A link to a heading adds its anchor, `/quickstart#connect-to-a-server`.

## Components

A page uses these components without an import:

| Component | Use |
| --- | --- |
| `<Callout>` | A note. `type="warn"` for a warning, `type="error"` for a danger. |
| `<Cards>`, `<Card>` | Links to other pages, as a grid. |
| `<Steps>`, `<Step>` | A procedure. Each step starts with a `###` heading. |
| `<Tabs>`, `<Tab>` | The same content for each platform or each method. |
| `<Video>` | A video. See below. |

A code block takes a title: ` ```bash title="Terminal" `. The
[Fumadocs documentation](https://fumadocs.dev/docs/markdown) lists the
other Markdown features.

## Screenshots and videos

Put each media file in `public/media/<page>/`, for example
`public/media/quickstart/setup.png`, and give it by its URL from the root.

A screenshot is a Markdown image. The build imports it, gives it its size
and a file name that changes with its content, so a browser keeps it in its
cache. The build does not resize it. A reader opens it larger with a click.
A missing file stops the build.

```mdx
![The setup window of the Client App](/media/quickstart/setup.png)
```

- Take the screenshot of one window, at a scale of 2, in the light theme.
- Save it as PNG, or as WebP for a photograph, at no more than 500 KB.
- Write the `alt` text as what the image shows.

A short clip with no sound is an animated screenshot. `loop` plays it
silently in a loop, with no controls:

```mdx
<Video src="/media/quickstart/drag.mp4" title="Drag a file into a conversation" loop />
```

A walkthrough with sound has controls and a poster image:

```mdx
<Video
  src="/media/quickstart/setup.mp4"
  poster="/media/quickstart/setup.png"
  title="The setup of the Client App"
  caption="The setup takes two minutes."
/>
```

- Encode a video as MP4 (H.264) at no more than 1080p.
- Keep a file in git at no more than 4 MB: a clip of 15 seconds or less.
- Put a longer video on a video host and give its URL as `src`.

## Checks

```bash
npm run typecheck
npm test             # each page has a title and a description, and each link resolves
npm run build        # compiles and exports each page to out/
npm run test:export  # serves out/ as Cloudflare does, and reads each address
```

`npm run build` fails when a part of the site needs a server, because the
export has none. `npm run test:export` starts the Worker of `wrangler.jsonc`
in the local runtime of Cloudflare. It reads each page, its Markdown copy,
its Open Graph image, the search index and the 404 page.

`cargo xtask dev` runs these checks for a change in `docs-site/`, and the
**docs site** job of CI runs them for each pull request
(`cargo xtask step docs-site-deps docs-site-typecheck docs-site-test docs-site-build docs-site-export`).
`cargo xtask advisories` audits `package-lock.json`.

## Agents

Each page has a Markdown copy for agents and for the **Copy Markdown**
button, at its URL with `.md` added, for example `/quickstart.md`. The
root page has its copy at `/index.md`. The build writes each copy under
`/llms.mdx/`, and `public/_redirects` serves it at the `.md` address.
`/llms.txt` lists the pages, and `/llms-full.txt` holds each page in one
file. The site sends no Markdown for a request that asks for it in the
`Accept` header, because that needs code on the server.

## Search

The build writes the search index to `/api/search` as one file. The
search dialog downloads it once and searches it in the browser.

## Deployment

The site is a Worker of static assets alone on Cloudflare
(`wrangler.jsonc`). It runs no code of its own. Each deployment follows the
release (`.github/workflows/docs.yml`):

| Event | Deployment |
| --- | --- |
| A pull request that changes the site | A preview version with the alias `pr-<number>`, at a `workers.dev` URL that the job summary shows. It does not change production. |
| A push to `main` | None |
| A `v*` tag | Production at docs.pagis.co, from the tree of the tag |

So docs.pagis.co shows the pages of the latest release, and a change to a
page shows there with the next release. To deploy a tag again, run the
**Docs** workflow and choose the tag in "Use workflow from". A pull request
from a fork gets no preview, because it cannot read the secrets.

### Set up Cloudflare and GitHub

Do these steps once. You need an administrator of the Cloudflare account,
access to the DNS of `pagis.co`, and an administrator of the GitHub
repository. The names in **bold** are the names on the screen.

#### 1. Put `pagis.co` on Cloudflare

A custom domain of a Worker must be in a zone of the same Cloudflare
account. If `pagis.co` is already a zone of the account, go to step 2.

1. In the [Cloudflare dashboard](https://dash.cloudflare.com), select
   **Add a domain**, type `pagis.co`, and select the **Free** plan.
2. Cloudflare copies the DNS records that it finds. Compare the list with
   the records at your current DNS provider, and add each record that is
   missing, especially the `MX` and `TXT` records of your mail.
3. At the registrar of `pagis.co`, replace the nameservers with the two
   nameservers that Cloudflare shows.
4. Wait until the zone shows **Active**. This can take up to 24 hours.
5. Make sure that no DNS record for `docs.pagis.co` exists. The first
   deployment makes that record, and it fails when a record is there.

#### 2. Turn on the workers.dev subdomain

The previews of the pull requests use a URL on `workers.dev`.

1. In the dashboard, open **Workers & Pages**.
2. If Cloudflare asks for a `workers.dev` subdomain, type one, for example
   `pagis`. Otherwise, the subdomain shows at the right of the page.

#### 3. Copy the account ID

Open **Workers & Pages**. The **Account ID** shows at the right of the
page. Copy it. `npx wrangler whoami` also prints it.

#### 4. Make an API token

1. Select the profile icon at the top right, then **My Profile** →
   **API Tokens** → **Create Token**.
2. Next to **Edit Cloudflare Workers**, select **Use template**.
3. Under **Account Resources**, select **Include** and your account.
4. Under **Zone Resources**, select **Include** → **Specific zone** →
   `pagis.co`.
5. Select **Continue to summary**, then **Create Token**.
6. Copy the token. Cloudflare shows it one time only.

If the first deployment fails with an authentication error for
`docs.pagis.co`, edit the token and add the permission **Zone** → **DNS** →
**Edit** for `pagis.co`.

#### 5. Add the secrets to GitHub

1. In the repository, open **Settings** → **Secrets and variables** →
   **Actions** → **New repository secret**.
2. Add `CLOUDFLARE_API_TOKEN` with the token of step 4.
3. Add `CLOUDFLARE_ACCOUNT_ID` with the account ID of step 3.

#### 6. Deploy the first time

The first deployment makes the Worker `pagis-docs` and the domain
`docs.pagis.co`. The previews of the pull requests work only after it.

- **With a release tag:** open **Actions** → **Docs** → **Run workflow**.
  In **Use workflow from**, select **Tags** and the latest `v*` tag, then
  **Run workflow**.
- **Without a tag:** deploy the current tree from your computer. Wrangler
  opens the browser to sign you in to Cloudflare. docs.pagis.co then shows
  this tree until the first tag deploys.

  ```bash
  cd docs-site
  npm ci
  npm run build
  npx wrangler login
  npx wrangler deploy
  ```

#### 7. Check the result

1. Open [docs.pagis.co](https://docs.pagis.co). The certificate can take a
   few minutes after the first deployment.
2. In the dashboard, open **Workers & Pages** → **pagis-docs** →
   **Settings** → **Domains & Routes**. It shows `docs.pagis.co` as a
   custom domain.
3. Open a pull request that changes a file in `docs-site/`. When the
   **preview** job of the **Docs** workflow completes, its summary shows
   the preview URL.
