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
| 1 | Client App and installer | macOS: a signed, notarized, stapled disk image, and a ZIP of the same app with its blockmap and the Update feed `latest-mac.yml`. Linux: an AppImage and a deb, with a checksum list that the release key and the Update Key sign, and the Update feeds `latest-linux.yml` and `latest-linux-arm64.yml`. All on the GitHub Release | macOS arm64, Linux amd64, Linux arm64 | The release |
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

**What the Headless Server image does not promise.** It is not notarized
and not stapled, because there is no platform gatekeeper to satisfy and no
Client App to be the trust root. Its trust root is the registry, the digest
a deployment pins, and the provenance attestation of that digest ("The
provenance attestations" below). It holds no Client Credential,
so no client trades a file for a Session against it (ADR-0024): a person
on a server signs in with an address and a password. It makes no promise
about two daemons against one database, which is not a supported shape.

## Building the Computer Image

```bash
docker login ghcr.io          # a token with write:packages
cargo xtask image --dry-run   # print the plan
cargo xtask image             # build each architecture, scan, push, and join
```

The daemon pulls `ghcr.io/pagis-co/pagis-computer` the first time an Agent
wakes, so an installation works only while the pinned tag is on GHCR. The
command builds `computer/` for `linux/amd64` and then for `linux/arm64` on
a buildx container builder that it makes one time. It scans the filesystem
of each architecture for secrets and for known vulnerabilities, and then
pushes that architecture by its digest, with no tag. Last, it joins both
digests into one multi-architecture index under the pin in
`crates/pagis-versions` (`docker buildx imagetools create`). It refuses to start when the
`org.pagis.computer.version` label in `computer/Dockerfile` and that pin
disagree, because the daemon reads the label back and boots nothing on a
mismatch.

The first publish of a version makes the GHCR package, which is private
until the account owner makes it public on the package page. The image
carries `org.opencontainers.image.source`, so the package links back to
this repository.

The architecture that the host does not run builds under emulation and
takes much longer than the native one, because the image compiles wlroots
and labwc from source. A release builds each architecture on a runner of
that architecture, so no build of the release runs under emulation. The
computer-image and computer-manifest stages of a release push the same
image when the registry does not hold its version yet, so a release needs no
separate publish. A published image version is never pushed again: every
release that pins it pulls the same bytes. A change to the image takes a
new version in `computer/Dockerfile` and in `crates/pagis-versions`.

## Building the Headless Server image

```bash
cargo xtask server-image            # build each architecture, scan, push, and join
cargo xtask server-image --dry-run  # print the plan and stop
```

The command builds the image for each architecture and exports its
filesystem, scans the export for secrets and for known vulnerabilities,
and pushes that architecture by digest only after both scans pass. Then
it joins both digests under the release tag ("The secret scan" and "The advisory checks" below). The push needs a Docker login to GHCR with
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
   host is not loopback. It upgrades the Debian packages of its base
   image before it installs its own, so each build holds the Debian
   security fixes that are out, also before Docker publishes a base image
   that holds them.

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
  for each change, and the release workflow runs the gate first. The scan
  does not read an untracked or ignored
  file, such as a local `.env`, because such a file does not enter the
  public tree.
- **Each image.** The publish builds the image for both architectures
  and exports the filesystem of each one into `dist/image-fs/`. The
  build writes one tar archive and the publish unpacks it, because the
  file-by-file `local` exporter of BuildKit can stop with no progress.
  gitleaks
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
- **npm audit** checks the lockfiles of `ui/`, which the server bundles,
  and `desktop/`, the Client App, at the high level. It reads the
  lockfile only. No release artifact holds the docs site, so only the
  daily check reads `docs-site/`.
- **Trivy** scans the filesystem of the Computer Image, of the Headless
  Server image and of each Server Package with its vulnerability scanner
  only, because gitleaks is the secret scan. The scan fails on a finding
  of high or critical severity that has a fixed version. A finding with no
  fixed version has no update to check. `.trivyignore` holds the reviewed
  exceptions.

The checks run apart from the gate, so a newly published advisory does
not block an unrelated pull request. They run in these places:

- The advisories stage of a release runs cargo-deny and npm audit after
  the gate and before any image build. Then Trivy scans each image for each architecture after its
  secret scan and before its push, and the
  package tree of each Server Package before it is signed, packed and
  published. `cargo xtask image` and `cargo xtask server-image` scan each
  image in the same way.
- `.github/workflows/advisories.yml` runs `cargo xtask advisories` each
  day and on a manual request. It runs cargo-deny and npm audit over the
  lockfiles of main, the docs site included, and Trivy over the Computer Image and the Headless
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

1. Update the dependency or the component, and run the check again. A
   Debian package of the Headless Server image takes its fix at the next
   build, because the image upgrades its packages.
2. When no update removes the advisory, find out how an installation can
   reach it. Record the exception with that reason and the update that you
   checked: an `ignore` entry with a `reason` in `deny.toml`, or the
   advisory ID in `.trivyignore` with a comment line above it that holds
   the reason. An advisory that an update removes is not an exception.

npm audit has no list of exceptions. When a parent package does not accept
the fixed version of a dependency, an `overrides` entry in `package.json`
selects it.

## The release workflow

A `v*` tag starts `.github/workflows/release.yml`. The tag names the
workspace version in `Cargo.toml` (`v0.1.0` for `0.1.0`), and a tag that
names another version stops the release in its first stage. To release:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The workflow runs `cargo xtask release <stage>` for the eight stages of the
server half, each in a job on the host it needs (`release_plan` in
`xtask/src/release.rs`). A stage stops at its first failed step, and the
workflow runs no job after a failed one. `cargo xtask release <stage>
--dry-run` prints the plan of one stage on any host. Without `--platform`,
the computer-image, server-image and linux stages build each architecture
in turn on one host.

Each release job that compiles saves its Rust cache, also after a failure.
A re-run of a job, or a new run on the same tag, compiles only what
changed. A cache of a tag run is visible only to runs on that tag, so the
first run on a new tag compiles the release builds from the start. The
client jobs run the same build as the packaging smoke jobs of the CI
workflow and share their cache, which main keeps warm.

Two third-party programs ship with a release, each pinned and each with its
license under `third_party/`: `gog` (MIT), the Google provider in every
server package, and uBlock Origin Lite (GPL-3.0-only), the content blocker
that the Chromium of the Computer Image loads.

## What the workflow needs

The repository administrator sets these once. `scripts/signing-secrets.sh`
walks the Apple Developer Account Holder through the signing secrets and sets
them with `gh`. The two PostHog secrets are set with `gh secret set`; a
release without them sends no analytics.

| Name | Where | What it holds |
| --- | --- | --- |
| `CSC_LINK` | Repository secret | The Developer ID Application certificate and its private key, a base64 `.p12` |
| `CSC_KEY_PASSWORD` | Repository secret | The password of the `.p12` |
| `APPLE_API_KEY_P8` | Repository secret | The text of the `.p8` file of an App Store Connect API key with the Developer role |
| `APPLE_API_KEY_ID` | Repository secret | The Key ID of that key |
| `APPLE_API_ISSUER` | Repository secret | The Issuer ID of the team |
| `PAGIS_POSTHOG_PROJECT_ID` | Repository secret | The PostHog project that a release build writes into the daemon (ADR-0026) |
| `PAGIS_POSTHOG_TOKEN` | Repository secret | The project token of that PostHog project |
| `PAGIS_RELEASE_GPG_PRIVATE_KEY` | Secret of the `release` environment | The private half of the Linux release key, with no passphrase |
| `PAGIS_UPDATE_SIGNING_KEY` | Secret of the `release` environment | The private half of the Update Key, an Ed25519 key in PKCS#8 PEM |

- **The `release` environment.** Each publication job waits in it until a
  required reviewer approves it. Make the maintainers its reviewers.
- **The release key and the Update Key.** Their public halves are
  `docs/release-key.asc` and `docs/update-key.pem`, and the tag must contain
  both (`docs/RELEASING-CLIENT.md`).
- **The packages.** The image jobs push with the job token, so the
  organization must let a workflow publish packages. The first push of each
  image makes a private GHCR package. The anonymous pull then stops the
  server-manifest job: make `pagis-computer` and `pagis-server` public on
  their package pages and run the failed job again.
- **The runners.** Each image job of the arm64 architecture runs on
  `ubuntu-24.04-arm`, the GitHub-hosted arm64 Linux runner.

A local run of a stage reads the same inputs from the machine: a Docker
login to GHCR with `write:packages` and `gh` signed in to the repository.
The macOS stage signs with `PAGIS_SERVER_SIGN_IDENTITY`, or with the one
Developer ID Application identity of the keychain when it is unset.
Notarization uses `PAGIS_NOTARY_KEYCHAIN_PROFILE` or
`APPLE_KEYCHAIN_PROFILE` when one is set, and else the API key in
`APPLE_API_KEY` (the path of the `.p8` file), `APPLE_API_KEY_ID` and
`APPLE_API_ISSUER`.

## The order

The workflow runs these jobs in this order, and ADR-0025 holds why:

1. **The gate.** The CI workflow runs on the tagged commit, and a red
   result stops the release. The gate scans the tracked files for secrets.
   Then `cargo xtask release advisories` checks the lockfiles of what the
   release ships ("The advisory checks" above), and a finding stops the
   release.
2. **The images.** Each image stage that builds runs as two jobs at the
   same time, one for each architecture, and each job runs on a Linux
   runner of that architecture (`--platform amd64` on `ubuntu-24.04`,
   `--platform arm64` on `ubuntu-24.04-arm`). A job hands the digest that
   it pushed to the manifest job as a workflow artifact.
   - `cargo xtask release computer-image --platform <platform>`. The
     stage builds the Computer Image
     for its architecture and exports its filesystem, scans the export
     for secrets, then for known vulnerabilities, and pushes the image by
     digest, with no tag, only after both scans pass, from the same build
     cache. When the registry already holds the pinned version, the stage
     skips the build and the push.
   - `cargo xtask release computer-manifest`. The stage joins the two
     digests into one index under the pinned tag, or skips when the
     registry already holds that tag. It then resolves the immutable
     digest of the tag to `dist/computer-image.txt`. The job attests the
     provenance of that digest.
   - `cargo xtask release server-image --platform <platform>`. The stage
     builds the Headless Server image against that digest for its
     architecture, and scans and pushes it by digest in the same way.
   - `cargo xtask release server-manifest`. The stage joins the two
     digests under the release tag. Last, it pulls the manifest of both
     images with an anonymous registry token, as a new person does. The
     job attests the provenance of the server image digest.
3. **The server packages.** Three jobs build them at the same time with the
   exact image reference, after the computer-manifest job and beside the
   server image jobs. Each builds the Product App, then `pagis` with
   the pinned, hash-checked cargo-auditable, assembles each target with its
   pinned, hash-checked `gog` and the notices, and scans each package tree
   for known vulnerabilities.
   - `cargo xtask release linux --platform <platform>` (Linux) builds the
     server package of one Linux architecture, and the workflow runs one
     job for amd64 and one for arm64. Both jobs run on an amd64 runner.
     The stage builds with `cross`, which runs `cargo build` in a Docker image of the target and
     runs no other cargo subcommand there. So the stage gives `cross` an
     image of its own (`CROSS_BUILD_DOCKERFILE`): the default image of the
     target, with cargo-auditable and a `cargo` that starts the `cargo` of
     the toolchain as `cargo auditable`. The images of `cross` are
     linux/amd64, and the Dockerfile names that platform, so an arm64 host
     builds and runs the image under emulation with no other setting. The
     stage packs the gzip tar archive of its architecture and writes its
     Runtime Lock (`dist/runtime-lock-linux-x64.json` or
     `dist/runtime-lock-linux-arm64.json`) from the finished archive and
     the files it extracts to.
   - `cargo xtask release macos` (macOS arm64) builds on the host. The job
     imports the certificate into a keychain of its own
     (`.github/scripts/import-signing-identity.sh`). The stage signs
     `pagis` and `gog`, builds the disk image, notarizes and staples it,
     and writes `dist/runtime-lock-darwin-arm64.json` from the final bytes.
4. **The draft** (`cargo xtask release draft`, macOS), after the server
   packages and the server-manifest job. Check every server
   package and every Runtime Lock. A missing target, a changed file, a
   wrong code signature or a mutable Computer image stops the release here.
   Then create the draft GitHub Release of the tag with the three server
   packages and the three locks, and nothing else. The job attests the
   provenance of each package and lock.
5. **The clients.** The macOS and Linux client jobs build their packages
   from the locks of the draft, sign the macOS one, and attest each package
   (`docs/RELEASING-CLIENT.md`).
6. **The publication.** The publication jobs wait in the `release`
   environment until a maintainer approves them. They upload the prepared
   client packages to the draft, and the last job publishes it.
7. **The documentation.** After the publication, the release workflow
   calls `.github/workflows/docs.yml`, which builds the documentation site
   from the tree of the tag and deploys it to docs.pagis.co. The Quickstart
   links to the client packages of the release, so the site goes live only
   when the release holds them.

A finding of the secret scan stops the release before the release
pushes anything that holds it. When the finding is a secret, revoke it
at its provider and make a new one before the next run ("The secret
scan" above). A finding of a vulnerability scan also stops the release
before the push or the publication ("The advisory checks" above).

## The provenance attestations

The workflow attests the provenance of every artifact it builds with
`actions/attest-build-provenance`: both image digests, each server package,
each Runtime Lock and each client package. An attestation is a Sigstore
signature of the digest, the workflow and the commit that built it. The
image attestations are also on the registry beside the image. To verify an
artifact:

```bash
gh attestation verify oci://ghcr.io/pagis-co/pagis-server:0.1.0 --repo pagis-co/pagis
gh attestation verify pagis-server-0.1.0-x86_64-unknown-linux-gnu.tar.gz --repo pagis-co/pagis
```

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
