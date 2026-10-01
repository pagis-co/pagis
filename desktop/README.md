# desktop/

The source of the Pagis Client App, an Electron application for macOS arm64
and for Linux amd64 and arm64. The pages of the
[Client App](https://docs.pagis.co/client-app) in the documentation site
(`docs-site/content/client-app/`) state what the Client App does for the
people who use it. [ADR-0025](../docs/adr/0025-the-client-app-installs-one-exact-server-or-connects-to-one.md)
holds the decisions. This document tells how to build, test and package it.

## Development

```bash
npm ci
npm run typecheck
npm test        # the lifecycle tests, against test/fake-daemon.mjs
npm run build   # the main process, and dist/design for the client's pages
```

The setup and status pages in `static/` read `static/pages.css` and the
design system of the Product App: `npm run build` copies `ui/src/tokens.css`,
the Inter font and the Pagis mark into `dist/design`, which the package
holds.

The tests need no server binary: they drive the supervisor against a fake
server script that answers the identity challenge, honours SIGINT, exits
with the restart code, and can hold a port.

To run setup from source, place the generated lock of your platform in the
`dist/` directory at the repository root, not in `desktop/dist/`:
`<repository>/dist/runtime-lock-<platform>-<arch>.json` (for example
`dist/runtime-lock-linux-x64.json`). `cargo xtask desktop` writes it there,
and `npm run build` cleans `desktop/dist/`. Then start Electron:

```bash
npm start
```

`PAGIS_HOME` names another state directory. `npm start -- --smoke` opens the
packaged local setup page against isolated client and state directories,
confirms that it loaded, and quits without starting a server.

## Packaging

```bash
npm run pack         # macOS arm64: the DMG, and the ZIP with latest-mac.yml
npm run pack:linux   # Linux amd64 and arm64: the AppImage and the deb
```

The GitHub publish configuration of `electron-builder.yml` puts
`app-update.yml` in the app, which names the releases of `pagis-co/pagis`
for electron-updater, and makes electron-builder write the Update feeds.
Both scripts pass `--publish never`: the release uploads each file itself.

`electron-builder.yml` puts the lock of each platform and architecture in
the package as `runtime-lock.json`, from the repository root's `dist/`:
`dist/runtime-lock-darwin-arm64.json`, `dist/runtime-lock-linux-x64.json` and
`dist/runtime-lock-linux-arm64.json`. A client with no lock says that this
build has no Server Runtime to install, and it can still connect to a
server.
It includes no server binary. On macOS it turns on the hardened runtime with
the audio-input and network entitlements, and notarizes when the Apple
credentials are present. On Linux it signs nothing: the release signs the
checksum list.

CI runs the same steps through one command, and so can you. A macOS host
runs the macOS plan and a Linux host the Linux plan; `--linux` names the
Linux plan on another host, for publication only:

```bash
cargo xtask desktop --dry-run   # print the plan
cargo xtask desktop             # require the locks, pack, check, --smoke
cargo xtask desktop --tag v0.1.0 --prepare
cargo xtask desktop --tag v0.1.0 --publish-existing
cargo xtask desktop --linux --tag v0.1.0 --publish-existing
```

On Linux the smoke runs the client of the host's architecture, under
`xvfb-run` when there is no display. The inventory checks both
architectures, and reads the deb's exact bytes with `dpkg-deb`.

On macOS, `--prepare` signs, notarizes, staples and checks the client. It stops before
publication, which waits for a maintainer's approval.
`--publish-existing` does not rebuild. It checks the exact DMG again,
downloads and compares the server tuple, pulls the Computer image digest with no credentials, and uploads
without overwrite. See
[`docs/RELEASING-CLIENT.md`](../docs/RELEASING-CLIENT.md), which also
holds the Linux release.

Signing can use `CSC_NAME` with a Developer ID Application fingerprint, or
`CSC_LINK` and `CSC_KEY_PASSWORD` for an imported certificate. Notarization
can use `APPLE_KEYCHAIN_PROFILE`, with optional `APPLE_KEYCHAIN`, or an App
Store Connect API key: `APPLE_API_KEY` (the path of its `.p8` file),
`APPLE_API_KEY_ID` and `APPLE_API_ISSUER`. A tag build fails when neither
complete route exists. The publication signs nothing, so it needs neither.

`desktop/scripts/check-package.mjs` holds the package to a size ceiling.
During setup, free disk space must cover the Client App, the locked server
package, the installed entries listed in the Runtime Lock, and, during a
repair, the damaged release that the repair replaces. The client cache can reuse a verified
package when the network is offline. It refuses a changed cache file.
