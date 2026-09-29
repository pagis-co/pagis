# Release the Client App

The server release must exist before the client can publish. Run
`cargo xtask release` first. It publishes the Computer image, the Headless
Server image, the server package of each client platform (the signed macOS
disk image and a Linux archive for each architecture), and one Runtime Lock
for each client platform:
`runtime-lock-darwin-arm64.json`, `runtime-lock-linux-x64.json` and
`runtime-lock-linux-arm64.json`. It does not build or publish a client.

The server release scans for secrets before anything becomes public
(`docs/RELEASING-SERVER.md`, "The secret scan"). Its gate scans the
tracked files. It then scans the filesystem of the Computer Image and of
the Headless Server image, and pushes each image only after a clean scan.
A finding stops the release. When a finding is a secret, revoke it at its
provider and make a new one. Removal from the tree is not enough, because
the secret stays valid.

The macOS client and the Linux client are released one after the other, and
each one in two phases. The split keeps the package bytes stable while a clean
machine tests them.

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
  by the validation below. The Headless Server image is published by the
  server release, so it is there before the client is.

## macOS

### Prepare the exact client

Download the server package and `runtime-lock-darwin-arm64.json` from the
release into `dist/`. Then run:

```bash
cargo xtask desktop --tag v0.1.0 --prepare
```

For a local keychain identity, set `CSC_NAME` to the fingerprint of a Developer
ID Application identity. Set `APPLE_KEYCHAIN_PROFILE` to the notarytool profile
name. Set `APPLE_KEYCHAIN` too when the profile is in a non-default keychain.
The server release also accepts these inputs. `PAGIS_SERVER_SIGN_IDENTITY` and
`PAGIS_NOTARY_KEYCHAIN_PROFILE` are the explicit server names.

CI can instead set `CSC_LINK` (the Developer ID Application certificate, a
base64 `.p12`), `CSC_KEY_PASSWORD`, `APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD`,
and `APPLE_TEAM_ID` as repository secrets. `scripts/signing-secrets.sh` walks
the account holder through the certificate, the `.p12` export and the
app-specific password, and sets the five secrets with `gh secret set`. The
release tools never export the key. Missing inputs stop a tag build. An Apple Development or Apple Distribution identity does not meet this
gate.

`--prepare` signs the outer DMG, submits it for notarization, and staples the
accepted ticket. It then mounts that exact DMG. The mounted app must contain
the same lock as `dist/runtime-lock-darwin-arm64.json`, and it must pass the client signature,
team, Gatekeeper, package inventory, compiled installer, and isolated setup
smoke checks. It does not publish.

### Record distribution proof

Move the prepared DMG to a clean local macOS account or test machine through a
download that sets ordinary quarantine. Keep Gatekeeper enabled. Do not remove
quarantine. Use no checkout and no developer `PATH`.

Record these values in `dist/distribution-proof.json`:

| Field | Required value |
| --- | --- |
| `schema` | `1` |
| `release` | The release without the `v` prefix |
| `runtime_lock_sha256` | SHA-256 of the exact Runtime Lock |
| `server_dmg_sha256` | SHA-256 named by that lock |
| `client_dmg_sha256` | SHA-256 of the prepared client DMG |
| `previous_client_dmg_sha256` | SHA-256 of the signed client used for the forward update |
| `team_id` | The Developer ID team in the lock and both apps |
| `notarization_submission_id` | The accepted client notarization submission |
| `macos_version` | The tested macOS version |
| `test_account` | A short name for the clean local account or machine |

The `checks` object must contain every key below. Set a key to `true` only when
the exact prepared bytes passed that check:

- `server_gatekeeper`
- `server_stapled`
- `server_designated_requirements`
- `client_gatekeeper`
- `client_stapled`
- `client_quarantined`
- `installed_runtime_quarantined`
- `initial_client_owned_launch`
- `authenticated_health`
- `product_app_opened`
- `keychain_created_and_read`
- `offline_client_owned_launch`
- `forward_update_read_same_keychain_item`
- `no_docker_setup`
- `computer_screen_ready`
- `computer_shell_ready`
- `workspace_reused_after_update`
- `unsafe_downgrade_refused`

Use `spctl --assess --type open --context context:primary-signature` and
`xcrun stapler validate` on both DMGs. Use `codesign --verify --strict` and
`codesign -dr -` on the installed `pagis` and `gog`. Confirm their quarantine
attributes after the client copies them. Start the server through the client,
check authenticated health and the Product App, then repeat the launch while
offline.

Create and read the keychain item that holds the Installation Key (service
`pagis`, account `safe-storage`) through the app on the clean account. Install the prepared forward update and confirm that
it reads the same item. Record the normal macOS access prompt if one appears.
A temporary `PAGIS_HOME` on a development account cannot prove this check.

Complete the local onboarding once without Docker. Test Computer screen and shell
readiness separately with Docker and the exact image digest in the Runtime
Lock. Reuse the existing Workspace through the forward update. Confirm that an
older signed server refuses the Workspace before it opens data.

### Publish the proven bytes

The server release that the lock names passed the secret scan of the
tree and of each image, because `cargo xtask release` publishes nothing
after a finding. Do not publish a client until each found secret is revoked
and replaced.

Copy the proof back beside the unchanged prepared package. Run:

```bash
cargo xtask distribution-proof validate \
  dist/distribution-proof.json \
  dist/runtime-lock-darwin-arm64.json \
  desktop/release/Pagis-0.1.0-arm64.dmg \
  0.1.0

cargo xtask desktop --tag v0.1.0 --publish-existing
```

The publish command does not rebuild or re-notarize. It mounts the exact DMG
again and repeats the local package checks against its app. It also compares
the local server package and Runtime Lock with the existing release, resolves the
immutable Computer image, and uploads the client without `--clobber`.
Missing, changed, unsigned, or unproven artifacts stop publication.

## Linux

Linux has no platform notary. The Pagis release key signs the checksum list of
the four packages, and the Runtime Lock inside each package pins the server
archive of its architecture (ADR-0025).

### The release key

The release key is an OpenPGP key that the maintainers hold. Its public half
is `docs/release-key.asc` in the repository, and the tag must contain it. Publication fails when the file is missing, and when the signature
does not verify with exactly that file. To make the key once:

```bash
gpg --quick-generate-key "Pagis release <release@example.invalid>" ed25519 sign 2y
gpg --armor --export <fingerprint> > docs/release-key.asc
```

Keep the private half off CI. Publication runs on a maintainer's machine, and
`gpg` reads the key from the local agent.

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
the `pagis-client-linux-v0.1.0` artifact. It signs and publishes nothing.

### Record distribution proof

Test each architecture on a clean Linux machine with a desktop session: an
Ubuntu 24.04 amd64 machine and an arm64 one. Download the packages as a person
would. Record one proof for each architecture in
`dist/distribution-proof-linux-x64.json` and
`dist/distribution-proof-linux-arm64.json`:

| Field | Required value |
| --- | --- |
| `schema` | `1` |
| `release` | The release without the `v` prefix |
| `platform` | `linux-x64` or `linux-arm64` |
| `runtime_lock_sha256` | SHA-256 of the exact Runtime Lock of that architecture |
| `server_archive_sha256` | SHA-256 of the server archive named by that lock |
| `appimage_sha256` | SHA-256 of the prepared AppImage |
| `deb_sha256` | SHA-256 of the prepared deb |
| `previous_client_sha256` | SHA-256 of the client package used for the forward update |
| `distribution` | The tested distribution and version |
| `test_account` | A short name for the clean account or machine |

The `checks` object must contain every key below, each `true` only when the
exact prepared bytes passed that check:

- `deb_installed`: `sudo apt install ./Pagis-<release>-<arch>.deb` installs,
  and the AppArmor profile loads.
- `appimage_launched`: the AppImage starts on a distribution that needs no
  AppArmor profile for it.
- `initial_client_owned_launch`, `authenticated_health`, `product_app_opened`:
  "Install on this computer" downloads the archive, starts the server, and
  opens the Product App signed in.
- `secret_service_key_created_and_read`: in a desktop session with GNOME
  Keyring or KWallet, the server log says the Installation Key is in the
  Secret Service, and the item (service `pagis`, account `safe-storage`)
  exists after a restart.
- `key_file_without_secret_service`: on a machine or session with no keyring
  daemon, the same run succeeds, the log names the Key File, and
  `~/.pagis/installation-key` has mode 600.
- `offline_client_owned_launch`: the launch repeats with the network off.
- `forward_update_read_same_key`: the forward update opens the same secrets.
- `no_docker_setup`, `computer_screen_ready`, `computer_shell_ready`,
  `workspace_reused_after_update`, `unsafe_downgrade_refused`: as on macOS.

### Publish the proven bytes

As on macOS, the server release that the locks name passed the secret
scan of the tree and of each image. Do not publish a client until each found
secret is revoked and replaced.

Put the four packages and the checksum list from the CI artifact in
`desktop/release/`, the proofs and both locks in `dist/`, and set
`PAGIS_RELEASE_GPG_KEY` to the fingerprint of the release key. On any host:

```bash
cargo xtask desktop --linux --tag v0.1.0 --publish-existing
```

It does not rebuild. It validates both proofs against the exact packages and
locks, compares the locks and archives with the release and resolves the
Computer image, checks every hash in the list, signs the list with the release
key into `Pagis-0.1.0-linux.SHA256SUMS.asc`, verifies that signature with
`docs/release-key.asc`, and uploads the four packages, the list and the
signature without `--clobber`.
