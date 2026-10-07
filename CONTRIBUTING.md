# Contributing to Pagis

Pagis is a Rust workspace (`crates/`), a React interface (`ui/`), an
Electron Client App (`desktop/`), a Capacitor Mobile App (`mobile/`), a
Computer Image (`computer/`) and a documentation site (`docs-site/`).
`xtask/` holds the checks and the release commands, which you run as
`cargo xtask <command>`.

Before you change code, read [AGENTS.md](AGENTS.md): it holds the
engineering rules of the project. [CONTEXT.md](CONTEXT.md) is the
glossary, and [docs/adr/](docs/adr) holds the architecture decisions.

## Prerequisites

- Rust stable, with `rustfmt` and `clippy`.
- [cargo-nextest](https://nexte.st), which runs the tests.
- Node.js at the version in [`.nvmrc`](.nvmrc), for `ui/`, `desktop/`,
  `mobile/` and `docs-site/`.
- Docker, for the Computer Image and the tests that start containers.
  Without Docker, the checks skip these tests.
- A JDK 21 and the Android SDK 36, with `ANDROID_HOME` set, for the Android
  tests of the Mobile App. Xcode 26 or later on macOS, for its iOS tests.
  Without one of them, the checks skip the tests that need it.

## Build and run

```bash
(cd ui && npm ci && npm run build)
cargo run -p pagis -- --local
```

`--local` makes a local installation: at the first run, the daemon prints
a one-time sign-in link and opens the browser on it. A debug build reads
`ui/dist` from disk, so a rebuilt interface shows without a new compile of
the daemon. A release build embeds it. [desktop/README.md](desktop/README.md)
tells how to build and run the Client App, and
[mobile/README.md](mobile/README.md) tells how to build and run the Mobile
App.

## Tests

```bash
cargo nextest run                           # every crate
cargo nextest run -p <crate> <test_name>    # one test
cargo nextest run -p <crate> <file_name>::  # each test in tests/<file_name>.rs
```

Most crates build one test binary from `tests/main.rs`, which declares each
file under `tests/` as a module. To add a test file to such a crate, put
the file in `tests/` and add `mod <file_name>;` to `tests/main.rs`.
`.config/nextest.toml` holds the test policy: one retry for a flaky test,
and a limit on the tests that start containers at the same time.

The tests that start containers are `#[ignore]`d, because they need
Docker. The checks below run them when Docker is reachable. They run in
the Computer Image of the tree, so build it first:

```bash
cargo xtask step computer-image
cargo nextest run -p pagis-computer --run-ignored only
```

The screen daemon, `computer/screend`, is a Cargo workspace of its own,
and it builds only on Linux. `cargo xtask step screend-test` runs its
tests in Docker, in the Rust image that builds it in the Computer Image.
The tests of the Exit Proxy that need a destination run against a
second container. The named volumes `pagis-screend-registry` and
`pagis-screend-target` keep the Cargo downloads and builds of these
containers.

In `ui/`, `desktop/`, `mobile/` and `docs-site/`, `npm run typecheck` and
`npm test` check the TypeScript code. In `docs-site/`, `npm run build` compiles and
exports each page, and `npm run test:export` serves the export as Cloudflare
does. [docs-site/README.md](docs-site/README.md) tells how to write a page. After a change to the HTTP API of `pagis-server`, run
`npm run api:generate` in `ui/` to write `openapi.json` and the API types
again.

### Tests that need an account

Two `#[ignore]` tests of `pagis-mail` reach a real mail account. The checks
never run them, because they need an account and they send mail or make a
mailbox. Without its variable, each one prints one line and does nothing.

```bash
PAGIS_LIVE_MAIL=1 \
PAGIS_LIVE_MAIL_ADDRESS=agent@example.com \
PAGIS_LIVE_MAIL_PASSWORD=... \
PAGIS_LIVE_MAIL_IMAP_HOST=imap.example.com \
PAGIS_LIVE_MAIL_SMTP_HOST=smtp.example.com \
cargo nextest run -p pagis-mail --test main --run-ignored only -E 'test(/^live_mail::/)'

PAGIS_LIVE_MIGADU=1 \
PAGIS_LIVE_MIGADU_DOMAIN=example.com \
PAGIS_LIVE_MIGADU_ACCOUNT=owner@example.com \
PAGIS_LIVE_MIGADU_API_KEY=... \
cargo nextest run -p pagis-mail --test main --run-ignored only -E 'test(/^live_migadu::/)'
```

The first test sends one message from a mailbox to itself and reads it
back. `PAGIS_LIVE_MAIL_IMAP_PORT` and `PAGIS_LIVE_MAIL_SMTP_PORT` change the
ports from 993 and 465. The second test makes one mailbox on a Migadu
domain, resets its password and deletes it.

## Checks

```bash
cargo xtask dev                # the checks of the current change
cargo xtask full               # every gate step
cargo xtask step <name>...     # the named gate steps, in order
cargo xtask advisories         # the dependency advisories
```

`dev` compares the branch with `origin/main`. It runs fmt, clippy and the
tests for each changed Rust package and each package that depends on it,
and the UI, desktop, Mobile App, Computer and documentation site checks for
a change in those parts. A change in `mobile/android/` or `mobile/ios/`
runs the native tests of that platform alone. A
change to shared build configuration, such as `Cargo.toml` or a workflow,
or to an unknown path, selects every gate step. A change to documents
only runs `git diff --check`. Each change also runs the secret scan.

`full` runs every gate step. `step` runs the gate steps that you name; an
unknown name prints the list. `dev` and `full` skip a native test of the
Mobile App whose toolchain is absent, and `step` fails it. The gate steps are:

| Step | What it checks |
| --- | --- |
| `fmt`, `clippy` | Formatting, and clippy with warnings as errors |
| `computer-image` | Builds the Computer Image that the Docker tests run |
| `screend-test` | The tests of `computer/screend`, in Docker |
| `test` | Each test of the workspace. With Docker, also the Docker tests |
| `gog-contract` | The Google adapter against the pinned `gog` release |
| `emergency-drift` | The emergency number table against its generator |
| `pins` | Each action, base image and Compose image is pinned (`cargo xtask pins --check`) |
| `contract-drift` | `openapi.json` and the API types of `ui/` match the server |
| `pagis-apt` | The shell tests of `computer/pagis-apt` |
| `ui-deps`, `ui-typecheck`, `ui-test` | The UI |
| `desktop-deps`, `desktop-typecheck`, `desktop-test` | The Client App |
| `docs-site-deps`, `docs-site-typecheck`, `docs-site-test`, `docs-site-build`, `docs-site-export` | The documentation site, and its export as Cloudflare serves it |
| `mobile-deps`, `mobile-typecheck`, `mobile-test` | The web part of the Mobile App |
| `mobile-android-test`, `mobile-ios-test` | The JUnit tests and the XCTest tests of the Mobile App, after `npx cap sync` |
| `secret-scan` | gitleaks over the tracked files |

The checks download the pinned gitleaks, `gog`, cargo-deny and Trivy
into the Cargo target directory, and check the SHA-256 of each archive.
`.gitleaks.toml` holds each secret scan exception with its reason.

`advisories` runs cargo-deny over the Cargo lockfiles, npm audit over the
npm lockfiles (the Mobile App included), and Trivy over the images of the latest release. The gate
does not run these checks, so a newly published advisory does not block
an unrelated pull request. A daily workflow and each release run them.
`deny.toml` and `.trivyignore` hold each reviewed exception with its
reason.

The emergency number table comes from libphonenumber at the tag in
`xtask/src/emergency.rs`. To take a newer release, change the tag and run
`cargo xtask emergency-numbers`.

## Continuous integration

`.github/workflows/ci.yml` runs on each pull request, in the merge queue,
and on `main`. Its jobs run at the same time, and each job runs a group of
gate steps with `cargo xtask step`:

| Job | Steps |
| --- | --- |
| fmt and clippy | `fmt clippy` |
| tests | `computer-image screend-test test` |
| contract drift | `ui-deps contract-drift gog-contract` |
| UI | `ui-deps ui-typecheck ui-test` |
| desktop | `desktop-deps desktop-typecheck desktop-test` |
| docs site | `docs-site-deps docs-site-typecheck docs-site-test docs-site-build docs-site-export` |
| Mobile App | `mobile-deps mobile-typecheck mobile-test mobile-android-test`, with a JDK 21 and the Android SDK of the runner |
| Mobile App (iOS) | `mobile-deps mobile-ios-test`, on macOS with Xcode 26 or later |
| pins, secrets and generated tables | `pins emergency-drift secret-scan pagis-apt` |
| packaging smoke (macOS), packaging smoke (Linux) | `cargo xtask desktop` with a fixture Runtime Lock |

The **CI success** job passes only when each other job passes. It is the
one check that a pull request needs. `.github/workflows/advisories.yml`
runs `cargo xtask advisories` each day. For a `v*` tag,
`.github/workflows/release.yml` prepares the Client App packages, and
after it publishes the release it calls `.github/workflows/docs.yml`,
which deploys the documentation site. A manual run of the docs workflow
deploys the site from `main`. That workflow also uploads a preview of the site for a pull request that changes it.
For a `push-relay-v*` tag, `.github/workflows/push-relay.yml` runs the
gate and publishes the Push Relay image (`docs/PUSH-RELAY.md`).

## Pull requests

- Write the test first. The tests stay in the repository.
- Use the terms of [CONTEXT.md](CONTEXT.md) in code, comments and titles.
- Write comments and documents in ASD-STE100 Simplified Technical English,
  in the present tense. They state the product as it is.
- Write each commit message as one present-tense sentence that states the
  result, for example "The setup window fits its tallest screen".
- Run `cargo xtask dev` before you push.

A pull request merges with a squash merge when CI passes.
