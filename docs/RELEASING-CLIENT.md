# Release the Client App

A `v*` tag starts the release workflow (`docs/RELEASING-SERVER.md`). Its
server half pushes the Computer image and the Headless Server image, and
creates the draft GitHub Release of the tag with the server package of each
client platform (the signed macOS disk image and a Linux archive for each
architecture) and one Runtime Lock for each client platform:
`runtime-lock-darwin-arm64.json`, `runtime-lock-linux-x64.json` and
`runtime-lock-linux-arm64.json`. The client jobs of the same run build the
clients from those locks.

The server release scans for secrets before anything becomes public
(`docs/RELEASING-SERVER.md`, "The secret scan"). Its gate scans the
tracked files. It then scans the filesystem of the Computer Image and of
the Headless Server image, and pushes each image only after a clean scan.
A finding stops the release. When a finding is a secret, revoke it at its
provider and make a new one. Removal from the tree is not enough, because
the secret stays valid.

Each client is released in two phases, so the published bytes are the
bytes that were signed and checked:

1. **Prepare.** The client jobs build and sign the exact packages, attest
   their provenance, and store them as artifacts of the run.
2. **Publish.** The publication jobs wait in the `release` environment until
   a maintainer approves them. They upload the prepared packages to the draft
   without overwrite, and the last job publishes the release.

The prepared packages are on the run page (the `client-macos` and
`client-linux` artifacts), and
`gh run download <run> --name client-macos --name client-linux` downloads
them.

## How the server artifacts relate to this release

A release is four artifacts, and `docs/RELEASING-SERVER.md` holds the matrix
and the version rule. What the client release depends on:

- **The server package** (the signed disk image on macOS, the gzip tar
  archive of the architecture on Linux) carries the same release number as the
  client. The Runtime Lock this document embeds names one server package by
  hash, so the two are one tuple and the client refuses any other.
- **The Computer image** carries a number of its own and is pinned by digest in
  the same lock. Every server of the release pulls it: the one this client
  installs, and the Headless Server a team runs on a VM.
- **The Headless Server image** carries the same release number as the
  client, and the client neither downloads nor verifies it. A Client App
  connected to a server holds a Compatibility Range instead of the lock: its
  own version and every later one that promises the same API. So the client
  and a Headless Server of the same release always work together, and a client
  keeps working against a server the Administrator upgraded.
- Publishing a client whose release has no matching server package is refused
  by the publication checks below. The Headless Server image is published by the
  server release, so it is there before the client is.

## macOS

### Prepare the exact client

The **prepare signed DMG** job downloads the server package and
`runtime-lock-darwin-arm64.json` from the draft into `dist/` and runs:

```bash
cargo xtask desktop --tag v0.1.0 --prepare
```

For a local keychain identity, set `CSC_NAME` to the fingerprint of a Developer
ID Application identity. Set `APPLE_KEYCHAIN_PROFILE` to the notarytool profile
name. Set `APPLE_KEYCHAIN` too when the profile is in a non-default keychain.
The server release also accepts these inputs. `PAGIS_SERVER_SIGN_IDENTITY` and
`PAGIS_NOTARY_KEYCHAIN_PROFILE` are the explicit server names.

CI instead reads `CSC_LINK` (the Developer ID Application certificate, a
base64 `.p12`), `CSC_KEY_PASSWORD`, and an App Store Connect API key:
`APPLE_API_KEY` is the path of its `.p8` file, which the job writes from the
`APPLE_API_KEY_P8` secret, with `APPLE_API_KEY_ID` and `APPLE_API_ISSUER`.
`scripts/signing-secrets.sh` walks the account holder through the
certificate, the `.p12` export and the API key, and sets the secrets with
`gh secret set`. The release tools never export the key. Missing inputs stop a tag build. An Apple Development or Apple Distribution identity does not meet this
gate.

`--prepare` signs the outer DMG, submits it for notarization, and staples the
accepted ticket. It then mounts that exact DMG. The mounted app must contain
the same lock as `dist/runtime-lock-darwin-arm64.json`, and it must pass the client signature,
team, Gatekeeper, package inventory, compiled installer, and isolated setup
smoke checks. It does not publish.

### Publish the prepared bytes

The server release that the lock names passed the secret scan of the
tree and of each image, because the release workflow publishes nothing
after a finding. Do not publish a client until each found secret is revoked
and replaced.

Approve the **publish the macOS client** job. The job puts the prepared DMG
in `desktop/release/`, the lock and the server package from the draft in
`dist/`, and runs:

```bash
cargo xtask desktop --tag v0.1.0 --publish-existing
```

The publish command does not rebuild, re-sign or re-notarize, so it needs no
signing input. It mounts the exact DMG
again and repeats the local package checks against its app. It also compares
the local server package and Runtime Lock with the draft, pulls the
immutable Computer image with no credentials, and uploads the client without
`--clobber`.
Missing, changed or unsigned artifacts stop publication.

## Linux

Linux has no platform notary. The Pagis release key signs the checksum list of
the four packages, and the Runtime Lock inside each package pins the server
archive of its architecture (ADR-0025).

### The release key

The release key is an OpenPGP key that the maintainers hold. Its public half
is `docs/release-key.asc` in the repository, and the tag must contain it. Publication fails when the file is missing, and when the signature
does not verify with exactly that file. `scripts/signing-secrets.sh` makes
the key once, with no passphrase:

```bash
gpg --batch --pinentry-mode loopback --passphrase '' \
  --quick-generate-key "Pagis release <release@example.invalid>" ed25519 sign 2y
gpg --armor --export <fingerprint> > docs/release-key.asc
```

The private half is the `PAGIS_RELEASE_GPG_PRIVATE_KEY` secret of the
`release` environment. Only a job of a `v*` tag in that environment reads
it, and each such job waits for a reviewer. The publication job signs with
no person present, which is why the key has no passphrase. The approval of
the environment protects it instead.

### Prepare the exact packages

On a `v*` tag, the **prepare Linux packages** CI job downloads
`runtime-lock-linux-x64.json` and `runtime-lock-linux-arm64.json` into `dist/`
and runs:

```bash
cargo xtask desktop --tag v0.1.0 --prepare
```

It builds the AppImage and the deb for amd64 and arm64 on one amd64 runner,
checks that each unpacked client and each exact deb carry the lock of their
architecture and no server, runs the compiled installer against a real
archive, smoke tests the amd64 client under a virtual display, and writes
`Pagis-0.1.0-linux.SHA256SUMS`. It uploads the four packages and the list as
the `client-linux` artifact, and attests the provenance of each package. It
publishes nothing, and it does not sign the checksum list.

### Publish the prepared bytes

As on macOS, the server release that the locks name passed the secret
scan of the tree and of each image. Do not publish a client until each found
secret is revoked and replaced.

Approve the **publish the Linux clients** job. The job puts the four packages
and the checksum list in `desktop/release/` and both locks from the draft in
`dist/`, imports the release key, sets `PAGIS_RELEASE_GPG_KEY` to its
fingerprint, and runs:

```bash
cargo xtask desktop --linux --tag v0.1.0 --publish-existing
```

It does not rebuild. It compares the locks and archives with the draft, pulls
the Computer image with no credentials, checks every hash in the list, signs
the list with the release key into `Pagis-0.1.0-linux.SHA256SUMS.asc`,
verifies that signature with `docs/release-key.asc`, and uploads the four
packages, the list and the signature without `--clobber`.
