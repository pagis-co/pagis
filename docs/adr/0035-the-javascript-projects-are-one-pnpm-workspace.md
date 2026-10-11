# 0035: The JavaScript projects are one pnpm workspace

Status: accepted.

## Context

The repository has five JavaScript projects: the Product App (`ui/`), the
Client App (`desktop/`), the Mobile App (`mobile/`), the documentation site
(`docs-site/`) and the Coding Harnesses of the Computer Image
(`computer/harnesses/`). With npm, nothing connects them. Each one has its
own `package-lock.json`:

| Project | Packages in the lock |
| --- | --- |
| `ui/` | 401 |
| `desktop/` | 400 |
| `mobile/` | 213 |
| `docs-site/` | 480 |
| `computer/harnesses/` | 270 |

The same dependency has a declaration in more than one project.
`@fontsource-variable/inter` and `@types/node` are in all four app
projects. `typescript`, `vitest` and `jsdom` are in three.
`@capacitor/core`, `@capacitor/app` and `@capacitor/browser` are in `ui/`
and `mobile/`, and the Product App and the Mobile App shell must use the
same Capacitor versions. Nothing keeps these versions equal. `docs-site/`
is on TypeScript 7 and Vitest 5, and the other app projects are on
TypeScript 5.9 and Vitest 4.

npm runs the install script of every dependency. Only the Computer Image
stops them, with `npm ci --ignore-scripts`. npm has no minimum age for a
new release, so an install can take a version that was published one
minute before. A compromised release that the registry removes soon after
publication can still reach an install.

Each worktree installs about 1,700 packages again into five `node_modules`
folders.

The Computer Image also ships npm and pnpm for the agents. These are tools
of the product, not of the repository.

Vite, Vue, Nuxt, Astro and Next.js are pnpm workspaces. Each one has one
lockfile at the root and one `pnpm-workspace.yaml`, which holds the
projects, the shared versions and the install rules.

## Decision

### pnpm 12 is the package manager of every JavaScript project

The root `package.json` sets `"packageManager": "pnpm@12.11.2"`. pnpm
reads this field and runs that version. pnpm 12 is a native program, so it
needs no Node to run itself. A Docker build installs the binary of its
architecture from the `@pnpm/exe.linux-<arch>` package of the registry,
pinned by sha512. 12.11.2 is the lowest version, because 12.11.0 fixes a
crash of pnpm 12 under QEMU emulation, as in a Docker build for another
architecture.

Corepack is not used. Node 25 and later do not ship it, and `.nvmrc` is
26.

### `ui/`, `desktop/`, `mobile/` and `docs-site/` are one workspace

The repository root holds `package.json`, `pnpm-workspace.yaml` and the
one `pnpm-lock.yaml`. Each step of `cargo xtask` installs and runs one
project with `--filter ./<project>`. All checkouts on a machine share one
pnpm store.

### `computer/harnesses/` is a pnpm project of its own

The Docker build context of the Computer Image is `computer/`, so the
image cannot read a lockfile at the root. `computer/harnesses/` keeps its
own `pnpm-workspace.yaml` and `pnpm-lock.yaml`, outside the root
workspace. It uses the pnpm version of the root `package.json`.

### The supply-chain rules live in `pnpm-workspace.yaml`

- `allowBuilds` names each dependency that has a build script, with `true`
  or `false` and a comment that gives the reason. `strictDepBuilds` stays
  `true`, the default, so a new build script stops the install until a
  person decides.
- `minimumReleaseAge: 1440` (one day). Because the value is set
  explicitly, `minimumReleaseAgeStrict` is `true`, and an install that
  needs a younger version fails. pnpm does not silently take an older
  version.
- `audit.ignore` holds each reviewed advisory exception, with a comment
  line that gives the reason, as `deny.toml` and `.trivyignore` do.
- pnpm 12 stops on a setting that it does not know, so a misspelt rule
  cannot be silently ignored.

### A catalog holds the shared versions

The default catalog of `pnpm-workspace.yaml` holds each dependency that
two or more projects of the workspace declare at the same major version.
A project refers to such a dependency as `"catalog:"`. A dependency that
two projects declare at different major versions stays out of the
catalog, and each project keeps its own range.

### The `node_modules` layout is isolated

The workspace uses the pnpm default, `nodeLinker: isolated`. A project can
import only the dependencies that its `package.json` declares. pnpm sets
the layout for the whole workspace only, so each project of the workspace
has the same layout. The workspace uses the default virtual store, not
`virtualStoreType: global`.

`computer/harnesses/` uses `nodeLinker: hoisted`. The harnesses are
third-party programs that their authors test with the flat layout of npm,
and they hold no code of the repository.

Other ways were considered:

- **npm workspaces.** They give one lockfile, but no catalog, no allowlist
  for build scripts, no minimum release age and no shared store.
- **Bun.** Its support for Electron packaging, Capacitor and audit is less
  mature.
- **Yarn with Plug'n'Play.** Electron, Capacitor and native tools do not
  work with it.
- **pnpm 11.** It has the same settings, lockfile format and store. Its
  TypeScript program gets bug fixes only, and new features go to pnpm 12.

## Consequences

- A worktree installs from the shared store in seconds.
- `cap sync` writes paths into `node_modules/.pnpm/` in the committed
  native files of the Mobile App. These paths hold the plugin versions, so
  a Capacitor update changes them.
- electron-builder packs the production dependencies of the Client App
  from the isolated layout. The packaging checks that `app.asar` holds
  each of them.
- `pnpm audit --filter` replaces `npm audit` in the advisory checks.
- The strict layout shows an import that `package.json` does not declare.
  The project then declares the dependency. It is not hoisted.
- The pnpm that builds the images does not stay in them. The pnpm that the
  Computer Image ships for the agents is a tool of the product, and this
  record does not choose its version.
