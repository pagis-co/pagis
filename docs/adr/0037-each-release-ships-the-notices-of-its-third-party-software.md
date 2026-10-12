# 0037: Each release ships the notices of its third-party software

Status: accepted.

## Context

The `pagis` program links several hundred third-party crates, and it
holds the Product App, which bundles npm packages. `pagis-screend` in the
Computer Image links more crates, and the Client App ships npm packages
in its app archive. Most of this software is under MIT, Apache-2.0, BSD,
ISC or Zlib. MIT, BSD and ISC require that a binary distribution carries
the copyright notice and the license text. Apache-2.0 also requires that
a distribution passes on the NOTICE file of the work. MPL-2.0 requires
that the source of the covered files stays available. A copy of the
SIL Open Font License font Inter must carry its license.

Established products solve this with a tool that reads the license of each
dependency and writes one notices file:

- [cargo-about](https://github.com/EmbarkStudios/cargo-about) of Embark
  Studios reads the crates that a program links for a set of targets,
  chooses an accepted license for each license expression, finds the
  license text in the crate source and fails when it cannot. Embark keeps
  it beside cargo-deny, and Zed generates its license file with it.
- [cargo-bundle-licenses](https://github.com/sstadick/cargo-bundle-licenses)
  writes the license texts of the dependencies to JSON, TOML or YAML. It
  has no list of accepted licenses and no target filter, and fewer people
  keep it.
- Vector and the Datadog Agent keep a generated `LICENSE-3rdparty` file
  in the repository, and CI fails when it does not match the lockfile.
- Vite writes the license of each npm package that a bundle holds with
  `build.license`.
- electron-builder copies the notices of Electron and of Chromium into
  each app, as `LICENSE.electron.txt` and `LICENSES.chromium.html`.

## Decision

### cargo-about reads the crates, and xtask writes the notices

cargo-about reads the license of each crate. `pagis-versions` pins its
version and the SHA-256 of its release archive for each host, as for
cargo-deny. It runs with `--frozen`, so it reads only the local crate
sources and its output depends only on the lockfile, and with `--fail`,
so a crate whose license it cannot satisfy or find fails the run.

`about.toml` is the configuration. Its `accepted` list holds the licenses
that Pagis can ship in a binary under the MIT License, in the order of
preference. Its `targets` are the release targets, the same as
`[graph] targets` of `deny.toml`. Build and dev dependencies do not go
into a binary, so cargo-about leaves them out. The crates of the
workspace and `pagis-screend` are the code of Pagis: each one sets
`publish = false`, and cargo-about leaves them out.

cargo-about writes JSON, and `cargo xtask notices` writes the notices
file from it. A handlebars template of cargo-about cannot read the NOTICE
file of a crate, and xtask adds each one. The file is plain text: for each
license text, the software that uses it and the text.

### The npm packages are checked against the same list

The Product App build writes the npm packages of its bundle, with their
licenses and license texts, to `ui/dist/.vite/license.json` and
`license-sw.json` (`build.license` of Vite). For the Client App, xtask
reads `pnpm licenses list --prod` and the license file of each package.
xtask checks the SPDX expression of each package against the `accepted`
list of `about.toml` with the `spdx` crate of Embark, which cargo-about
uses. A package with no license file gets the standard SPDX text of its
license, as cargo-about does for a crate.

### Each release artifact holds its notices

| Artifact | File | Written |
| --- | --- | --- |
| Server Package | `THIRD_PARTY_NOTICES` beside `pagis` | by the release, after the Product App build |
| Headless Server image | `/usr/local/lib/pagis/THIRD_PARTY_NOTICES` | by the image build, after `pagis` |
| Computer Image | `/usr/share/doc/pagis-screend/THIRD_PARTY_NOTICES` | stored as `computer/screend/THIRD_PARTY_NOTICES` |
| Client App | `THIRD_PARTY_NOTICES` in the resources of the app | by `cargo xtask desktop`, before the pack |

The notices of the Server Package and of the Headless Server image are
one file: gog, the crates of `pagis` for the three release targets, and
the npm packages of the Product App. The release writes them, so a
dependency change needs no extra commit.

The Computer Image builds from `computer/` alone, where xtask is not.
So the notices of `pagis-screend` are stored beside it, and the gate
checks that the stored file is what `cargo xtask notices screend` writes,
as it checks the emergency number table. Its lockfile changes seldom.

The Client App also holds the notices of Electron and of Chromium, which
electron-builder copies, and the license of Inter beside the font that its
pages load. `check-package.mjs` refuses a package without them.

### The gate checks the notices

The gate step `notices` builds the Product App, writes the notices of the
Server Package and of the Client App into the target directory, and
checks the stored notices of `pagis-screend`. It fails on a license that
`about.toml` does not accept, on a crate or a package with no license
text, and on a missing Product App license list. CI runs it in the
contract job, beside the steps that also need the packages of the UI.

### What the notices do not cover

These parts ship with their own license files, or have no notices from
Pagis:

- The other programs of the Computer Image: labwc and wlroots, built from
  source; Node, uv and OpenCode; the npm packages of the Coding Harnesses,
  which keep their license files in their directories; and the Debian
  packages, which keep theirs under `/usr/share/doc`. The Computer Image
  does not hold the `LICENSE` of the Node release, and it gives no
  written offer of the source of labwc.
- The Push Relay image, which links crates of the workspace.
- The Mobile App, which holds the Product App and Capacitor.

## Consequences

- A crate or an npm package with a new license fails the gate until a
  maintainer adds the license to `accepted` in `about.toml`, after a check
  that Pagis can ship it.
- A change of `computer/screend/Cargo.lock` needs `cargo xtask notices
  screend` in the same change.
- The release, the image build and the gate download the pinned
  cargo-about. The image build compiles xtask to run it.
- One Server Package names the crates of all three release targets, also
  those that only another target links.
