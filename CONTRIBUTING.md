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
- pnpm 12, for the pnpm workspace at the repository root. Install it once
  with `npm install -g pnpm@12`. pnpm then runs the version that the
  `packageManager` field of the root `package.json` names.
- Docker, for the Computer Image and the tests that start containers.
  Without Docker, the checks skip these tests.
- A JDK 21 and the Android SDK 36, with `ANDROID_HOME` set, for the Android
  tests of the Mobile App. Xcode 26 or later on macOS, for its iOS tests.
  Without one of them, the checks skip the tests that need it.

## Build and run

```bash
pnpm install --frozen-lockfile --filter ./ui
pnpm --filter ./ui run build
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

`pnpm --filter ./ui run typecheck` and `pnpm --filter ./ui run test` check
the TypeScript code of the Product App, and `pnpm --filter ./desktop run
typecheck` and `pnpm --filter ./desktop run test` the code of the Client
App. `--filter ./mobile` does the same for the Mobile App.
`pnpm --filter ./docs-site run typecheck` and
`pnpm --filter ./docs-site run test` check the documentation site,
`pnpm --filter ./docs-site run build` compiles and exports each page, and
`pnpm --filter ./docs-site run test:export` serves the export as Cloudflare
does. [docs-site/README.md](docs-site/README.md) tells how to write a page. After a change to the HTTP API of `pagis-server`, run
`pnpm --filter ./ui run api:generate` to write `openapi.json` and the API
types again.

### Portable crate checks under Node

`cargo xtask step wasm32` runs clippy for `wasm32-unknown-unknown` and
runs the `wasm-bindgen-test` tests under Node. `WASM_CRATES` in
`xtask/src/wasm32.rs` lists the crates and their test binaries:
`pagis-core`, `llm-router` and `pagis-broker`. A crate joins the list when
it builds and runs on the target. It does not leave the list.

The step needs the rustup target, `wasm-bindgen-test-runner` at the
version of `wasm-bindgen` in `Cargo.lock`, a clang with the WebAssembly
back end, `llvm-ar` and Node at the version in `.nvmrc`. It checks these
tools before each Cargo command and names a missing tool.
`wasm-bindgen-cli` supplies the runner, which `.cargo/config.toml`
selects for this target. The runner uses Node.

```bash
rustup target add wasm32-unknown-unknown
rustup component add llvm-tools
version=$(cargo tree --locked -p wasm-bindgen --target wasm32-unknown-unknown --depth 0 --prefix none | sed -n 's/^wasm-bindgen v//p')
cargo install wasm-bindgen-cli --version "$version" --locked
cargo xtask step wasm32
```

On Linux, install clang with the system package manager. On macOS,
`/usr/bin/clang` (Apple clang) supports the target. `clang --print-targets`
lists `wasm32` when the compiler has the WebAssembly back end. The step
sets `CC_wasm32_unknown_unknown` to clang. It finds `llvm-ar` on `PATH`
or in the `llvm-tools` component at
`$(rustc --print sysroot)/lib/rustlib/<host>/bin/llvm-ar`, and sets
`AR_wasm32_unknown_unknown` to that path. The macOS `ar` does not make
an archive of WebAssembly objects that the wasm linker can use.

The Bash parser tests use the production parser and the same expected
simple commands as the native tests. This target uses the wasm headers
and allocator of tree-sitter. It needs no WASI SDK.
`cargo nextest run -p pagis-broker --lib rules::tests::` runs the same
rule tests natively. The native test harness dependencies build only
for native targets.

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
and selects `wasm32` when a changed package is in `WASM_CRATES` or is a
dependency of a crate in that list. It runs the UI, desktop, Mobile App,
Computer and documentation site checks for a change in those parts. A change in `mobile/android/` or `mobile/ios/`
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
| `members-alone` | `cargo check -p <member> --all-targets` for each workspace member, with a separate feature graph |
| `wasm32` | Target clippy and Node tests of each crate in `WASM_CRATES` |
| `computer-image` | Builds the Computer Image that the Docker tests run |
| `screend-test` | The tests of `computer/screend`, in Docker |
| `test` | Each test of the workspace. With Docker, also the Docker tests |
| `gog-contract` | The Google adapter against the pinned `gog` release |
| `emergency-drift` | The emergency number table against its generator |
| `pins` | Each action, base image and Compose image is pinned (`cargo xtask pins --check`) |
| `contract-drift` | `openapi.json` and the API types of `ui/` match the server |
| `notices` | Each shipped crate, npm package and Swift package has an accepted license and a notice, and the stored notices of screend and of the Coding Harnesses are current (`cargo xtask notices`) |
| `pagis-apt` | The shell tests of `computer/pagis-apt` |
| `ui-deps`, `ui-typecheck`, `ui-test` | The UI |
| `desktop-deps`, `desktop-typecheck`, `desktop-test` | The Client App |
| `docs-site-deps`, `docs-site-typecheck`, `docs-site-test`, `docs-site-build`, `docs-site-export` | The documentation site, and its export as Cloudflare serves it |
| `mobile-deps`, `mobile-typecheck`, `mobile-test` | The web part of the Mobile App |
| `mobile-android-test`, `mobile-ios-test` | The JUnit tests and the XCTest tests of the Mobile App, after `cap sync` |
| `secret-scan` | gitleaks over the tracked files |

The checks download the pinned gitleaks, `gog`, cargo-deny and Trivy
into the Cargo target directory, and check the SHA-256 of each archive.
`.gitleaks.toml` holds each secret scan exception with its reason.

`advisories` runs cargo-deny over the Cargo lockfiles, pnpm audit over the
pnpm lockfile (the Mobile App included), and Trivy over the images of
the latest release. The gate
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
| fmt and clippy | `fmt clippy members-alone` |
| tests | `computer-image screend-test test` |
| contract drift and notices | `ui-deps desktop-deps mobile-deps contract-drift gog-contract notices` |
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
For a `mobile-v*` tag, `.github/workflows/mobile-release.yml` runs the
gate, prepares the iOS app of the Mobile App, and uploads it to App Store
Connect (`docs/RELEASING-MOBILE.md`).

## Pull requests

- Write the test first. The tests stay in the repository.
- Use the terms of [CONTEXT.md](CONTEXT.md) in code, comments and titles.
- A reverse-DNS identifier starts with `co.pagis`, after the project domain
  `pagis.co`: a bundle ID, an application ID, an App Group, a code signing
  identifier, a container label and a Java package.
- Write comments and documents in ASD-STE100 Simplified Technical English,
  in the present tense. They state the product as it is.
- Write each commit message as one present-tense sentence that states the
  result, for example "The setup window fits its tallest screen".
- Run `cargo xtask dev` before you push.

A pull request merges with a squash merge when CI passes.
