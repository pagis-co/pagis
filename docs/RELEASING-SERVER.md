# Releasing the server, and the four artifacts

A release of Pagis is four artifacts. Two installation methods are built
from them:

- the **Client App**, which installs and supervises a Server Runtime on the
  person's own machine, or connects to a server;
- the **Headless Server**, where a team runs the same daemon on a VM.

Both use one Computer image.

`docs/RELEASING-CLIENT.md` holds the client half of the release. ADR-0025
holds the decisions.

## The matrix

| | Artifact | Published as | Platforms | Number it carries |
| --- | --- | --- | --- | --- |
| 1 | Client App and installer | macOS: a signed, notarized, stapled disk image. Linux: an AppImage and a deb, with a checksum list the release key signs. All on the GitHub Release | macOS arm64, Linux amd64, Linux arm64 | The release |
| 2 | Computer image | `ghcr.io/pagis-co/pagis-computer:<image version>` | linux/amd64, linux/arm64 | Its own image version |
| 3 | Server package | The package the Client App downloads. On macOS it is a signed, notarized, stapled disk image. On Linux it is a gzip tar archive for each architecture. A Runtime Lock for each client platform names its package by size and SHA-256 | macOS arm64, Linux amd64, Linux arm64 | The release |
| 4 | Headless Server image | `ghcr.io/pagis-co/pagis-server:<release>` | linux/amd64, linux/arm64 | The release |

## The version rule

**One release is one number, and artifacts 1, 3 and 4 carry it.** The
client, the server package it downloads and the Headless Server image of one release are built from one commit and tagged
with one version. A release never publishes some of them without the
others.

**The Computer image has a number of its own.** It changes on its own
schedule, because the browser and the desktop inside it are not the
daemon. A release pins one version of it and resolves it to an immutable
digest, and every server of the release pulls that same image. The
Computer image is the one artifact of a release whose number may differ
from the rest.

**What a mismatch does, in each of the five places one can happen:**

| Pair | The rule | What a mismatch does |
| --- | --- | --- |
| Client App and the server package it downloads | The Runtime Lock: one release, one platform, one package, one Computer image, no adapter | The client refuses the package and installs nothing |
| Client App and a server it connects to | The Compatibility Range: the client's own version and every later one that promises the same API | The client refuses the server and says which end to update |
| Server and Computer image | The daemon compares the image's `org.pagis.computer.version` label with its own pin | No container boots, and the daemon says which two versions it saw |
| Server and the data it opens | The release marker under the state directory is a one-way high-water mark | A server older than the data refuses to open it, before it migrates anything |
| Headless Server image and Postgres | The image carries the PostgreSQL client that `pagis backup` runs | A client older than the server refuses to dump it; keep the image's major version and the database's equal |

**What the Headless Server image does not promise.** It is not signed, not
notarized and not stapled, because there is no platform gatekeeper to
satisfy and no Client App to be the trust root. Its trust root is the
registry and the digest a deployment pins. It holds no Client Credential,
so no client trades a file for a Session against it (ADR-0024): a person
on a server signs in with an address and a password. It makes no promise
about two daemons against one database, which is not a supported shape.

## Building the Computer Image

```bash
docker login ghcr.io          # a token with write:packages
cargo xtask image --dry-run   # print the plan
cargo xtask image             # build both architectures, scan, and push
```

The daemon pulls `ghcr.io/pagis-co/pagis-computer` the first time an Agent
wakes, so an installation works only while the pinned tag is on GHCR. The
command builds `computer/` for `linux/amd64` and `linux/arm64` on a buildx
container builder that it makes one time, scans the filesystem of each for
secrets and for known vulnerabilities, and pushes both under the pin in
`crates/pagis-versions`. It refuses to start when the
`org.pagis.computer.version` label in `computer/Dockerfile` and that pin
disagree, because the daemon reads the label back and boots nothing on a
mismatch.

The first publish of a version makes the GHCR package, which is private
until the account owner makes it public on the package page. The image
carries `org.opencontainers.image.source`, so the package links back to
this repository.

The architecture that the host does not run builds under emulation and
takes much longer than the native one, because the image compiles wlroots
and labwc from source. `cargo xtask release` pushes the same image as one
of its steps, so a release needs no separate publish.

## Building the Headless Server image

```bash
cargo xtask server-image            # build both architectures, scan, and push
cargo xtask server-image --dry-run  # print the plan and stop
```

The command builds the image and exports its filesystem, scans the
export for secrets and for known vulnerabilities, and pushes only after
both scans pass ("The secret scan" and "The advisory checks" below). The push needs a Docker login to GHCR with
`write:packages`. The command refuses to publish an image whose
`org.pagis.server.version` label is not the workspace version, because
the tag and the label are the same claim made twice.

The build is the `Dockerfile` at the repository root, with the whole
repository as its context:

1. The `ui` stage builds the bundle the daemon serves, which is one Vite
   package with two entry points: the Product App and the
   Administration Interface.
2. The `build` stage compiles `pagis` for Linux with `cargo auditable`,
   with that bundle embedded. A release passes the immutable Computer
   image digest in `PAGIS_COMPUTER_IMAGE`; a plain build keeps the tag
   `pagis-versions` pins.
3. The `gog` stage fetches the pinned Google Connection runner and
   checks the same upstream hash the macOS package checks.
4. The final stage carries the daemon, `gog` beside it, the upstream
   licenses and notices, and every program the daemon starts. It starts the
   daemon without `--local` and sets `PAGIS_REQUIRE_PUBLIC_ORIGIN`, so the
   daemon refuses to start with `--local` or without a Public Origin whose
   host is not loopback.

**Every program the daemon starts is in the image**, so a Computer, a
memory repack or a git Plugin does not fail on a VM that carries nothing
else:

| Program | What starts it | Where the image gets it |
| --- | --- | --- |
| `gog` | Google Connections | The `gog` stage, pinned and hash-checked |
| `git` | Best-effort memory repacks and git Plugin installs | `git` from Debian |
| `pg_dump`, `pg_restore` | `pagis backup` and `pagis restore` | `postgresql-client-18` from the PostgreSQL apt repository |
| `curl` | The container health check, and nothing the daemon starts | `curl` from Debian |

The daemon starts nothing else. It reaches Docker over the socket with a
library and never through the `docker` command. When a port is taken, it
names the port and the flag that moves it, and it does not ask `lsof`
for the process. The Client App names that process on its setup page,
from the machine it runs on.

## The secret scan

Three things become public: the tree of the public repository, the
Computer Image and the Headless Server image. Anyone can read the tree and
pull the images. A secret in one of them gives the reader the service
behind it, and the secret stays valid when the published copy is deleted.
So gitleaks scans each of them before it becomes public:

- **The tree.** The gate scans the tracked files as the working tree
  holds them. `cargo xtask full` runs the scan, `cargo xtask dev` runs it
  for each change, and `cargo xtask release` runs the gate first. The scan
  does not read an untracked or ignored
  file, such as a local `.env`, because such a file does not enter the
  public tree.
- **Each image.** The publish builds the image for both architectures
  and exports the filesystem of each one into `dist/image-fs/`. gitleaks
  scans the text files of the export. The push comes only after a clean
  scan, and it takes each layer from the build cache of the scanned
  build. The push step then removes the export. The release,
  `cargo xtask image` and `cargo xtask server-image` all publish this way,
  and each one stops at the first step that fails.

`pagis-versions` pins the version of gitleaks and the SHA-256 of its
archive for each host. The scan checks the archive against the pinned
hash before each run, and keeps it in the target directory.

The scan fails on every finding. `.gitleaks.toml` holds the default rules
of gitleaks and the allowlist. When a finding is not a secret, add an
entry to the allowlist, with the reason in its `description`. An entry
for a file in an image names the path of the file in the export, as
`^dist/image-fs/computer/linux_(?:amd64|arm64)/<path>`, and in
`targetRules` only the rules that flag it, so the other rules still scan
that file. The scan
ignores `gitleaks:allow` comments and refuses a `.gitleaksignore` file,
because they hold no reason. gitleaks prints each finding with the secret
redacted.

**When a finding is a secret, revoke it at its provider and make a new
one.** Then remove it from the tree. Removal from the tree is not enough.
The secret stays valid, and a clone, a log or a pushed layer can hold a
copy of it. Do this also when the secret did not become public.

## The advisory checks

Three tools check the dependencies of a release against the published
advisories:

- **cargo-deny** checks the Cargo lockfiles of the workspace and
  `computer/screend` for the targets that a release ships. `deny.toml` holds the targets and the ignored advisories.
- **npm audit** checks the lockfiles of `ui/` and `desktop/` at the high
  level. It reads the lockfile only.
- **Trivy** scans the filesystem of the Computer Image, of the Headless
  Server image and of each Server Package with its vulnerability scanner
  only, because gitleaks is the secret scan. The scan fails on a finding
  of high or critical severity that has a fixed version. A finding with no
  fixed version has no update to check. `.trivyignore` holds the reviewed
  exceptions.

The checks run apart from the gate, so a newly published advisory does
not block an unrelated pull request. They run in these places:

- `cargo xtask release` runs the advisory checks before it builds. Then
  Trivy scans each image after its secret scan and before its push, and the
  package tree of each Server Package before it is signed, packed and
  published. `cargo xtask image` and `cargo xtask server-image` scan each
  image in the same way.
- `.github/workflows/advisories.yml` runs `cargo xtask advisories` each
  day and on a manual request. It runs cargo-deny and npm audit over the
  lockfiles of main, and Trivy over the Computer Image and the Headless
  Server image of the latest release, for both architectures. The Runtime
  Lock of the release names its Computer Image. When a check fails, the
  workflow opens an issue that names each advisory in the output of the
  failed checks. When an open issue of the workflow exists, it adds a
  comment to that issue.

`pagis-versions` pins the versions of cargo-deny and Trivy and the
SHA-256 of their archives for each host. Each check verifies the archive
against the pinned hash before each run, and keeps it in the target
directory.

**What Trivy identifies.** Trivy identifies the Debian packages, `uv`,
npm, pnpm, the npm packages that Node ships, `gog`, and the crates of
`pagis-screend` and of `pagis`. Each build of `pagis-screend` and of
`pagis` is a `cargo auditable` build, which writes the list of the crates
into the executable. `pagis-versions` pins the version of cargo-auditable,
and both Dockerfiles install that version.

**What Trivy does not identify.** Trivy does not identify labwc and
wlroots, which `computer/Dockerfile` builds from source, or the Node
runtime, whose executable holds no package metadata. The scan of the
Computer Image prints their pinned versions. Before each release, compare
each one by hand with the security advisories of its upstream project.

**When a check fails**, find the update that removes the advisory first:

1. Update the dependency or the component, and run the check again.
2. When no update removes the advisory, find out how an installation can
   reach it. Record the exception with that reason and the update that you
   checked: an `ignore` entry with a `reason` in `deny.toml`, or the
   advisory ID in `.trivyignore` with a comment line above it that holds
   the reason. An advisory that an update removes is not an exception.

npm audit has no list of exceptions. When a parent package does not accept
the fixed version of a dependency, an `overrides` entry in `package.json`
selects it.

## The release command

```bash
cargo xtask release --dry-run  # print the plan
cargo xtask release            # gate, build, publish
```

The command needs Docker for `cross` and the images, `gh` signed in to the
repository, a Docker login to GHCR, and the macOS release tools. Set
`PAGIS_SERVER_SIGN_IDENTITY` to a Developer ID Application identity, or
leave it unset when the keychain holds one such identity. Notarization
uses `PAGIS_NOTARY_KEYCHAIN_PROFILE` when it is set, and else `APPLE_ID`,
`APPLE_APP_SPECIFIC_PASSWORD` and `APPLE_TEAM_ID`. The release version is
the workspace version in `Cargo.toml`. The Computer Image tag is the pin in
`pagis-versions`.

Two third-party programs ship with a release, each pinned and each with its
license under `third_party/`: `gog` (MIT), the Google provider in every
server package, and uBlock Origin Lite (GPL-3.0-only), the content blocker
that the Chromium of the Computer Image loads.

## The order

`cargo xtask release` runs these steps in this order (`release_plan` in
`xtask/src/release.rs`), and ADR-0025 holds why. It stops at the first
step that fails, so no later step runs:

1. **The gate and the advisory checks.** `cargo xtask full` and the
   advisory checks run first, and a red result stops the release. The gate
   scans the tracked files for secrets.
2. **The Computer Image.** Build it for both architectures and export its
   filesystem. Scan the export for secrets, then for known
   vulnerabilities. Push the image only after both scans pass, from the
   same build cache, then resolve its immutable digest.
3. **The Headless Server image.** Build it for both architectures against
   that digest and export its filesystem. Scan the export for secrets,
   then for known vulnerabilities, and push the image only after both
   scans pass. It comes before anything is signed, so a release that
   cannot produce it stops early.
4. **The anonymous pull.** Pull the manifest of the Computer image and of
   the Headless Server image with an anonymous registry token, as a new
   person does. A registry that refuses stops the release: make the
   package public on GHCR and run the release again.
5. **The builds.** Build the Product App, then build `pagis` for macOS
   arm64, Linux amd64 and Linux arm64 with that exact image reference, and
   assemble each target with its pinned, hash-checked `gog` and the
   notices. Each build is a `cargo auditable` build with the pinned,
   hash-checked cargo-auditable. The macOS build runs it on the host. The
   Linux builds run `cross`, which runs `cargo build` in a Docker image of
   the target and runs no other cargo subcommand there. So the release
   gives `cross` an image of its own (`CROSS_BUILD_DOCKERFILE`): the default
   image of the target, with cargo-auditable and a `cargo` that starts the
   `cargo` of the toolchain as `cargo auditable`. The images of `cross`
   are linux/amd64, and the Dockerfile names that platform, so an arm64
   host builds and runs the image under emulation with no other setting.
   Scan each assembled package tree for known vulnerabilities.
6. **The macOS server package.** Sign `pagis` and `gog`, build the macOS
   arm64 disk image, notarize and staple it, and write
   `dist/runtime-lock-darwin-arm64.json` from the final bytes.
7. **The Linux server packages.** Pack the gzip tar archive of each Linux
   architecture.
8. **The Linux locks.** Write `dist/runtime-lock-linux-x64.json` and
   `dist/runtime-lock-linux-arm64.json` from the finished archive of each
   architecture and the files it extracts to.
9. **Validation.** Check every server package and every Runtime Lock. A
   missing target, a stale Product App, a changed file, a wrong code
   signature or a mutable Computer image stops the release here.
10. **Publish.** Create the GitHub Release with the three server packages
    and the three locks, and nothing else.

A finding of the secret scan stops the release before the release
pushes anything that holds it. When the finding is a secret, revoke it
at its provider and make a new one before the next run ("The secret
scan" above). A finding of a vulnerability scan also stops the release
before the push or the publication ("The advisory checks" above).

The command does not build or publish the Client App. A `v*` tag starts
the CI jobs that prepare the macOS and Linux clients, and each client
publishes only after its exact bytes pass the clean-machine proof in
`docs/RELEASING-CLIENT.md`.

## Private vulnerability reporting

`SECURITY.md` names GitHub private vulnerability reporting as the only
route to report a vulnerability. A repository administrator turns it on in
the repository **Settings**, under **Advanced Security**. Before each
release, check it:

```bash
gh api repos/pagis-co/pagis/private-vulnerability-reporting
```

The answer must show `"enabled": true`. Any other answer, or a 404, means
that nobody can report a vulnerability privately. Do not publish a release
then.
