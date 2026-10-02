# 0025: The Client App installs one exact server or connects to one

Status: accepted.

## Context

A team runs the Headless Server on a VM and reaches it from a browser or a
Client App. A person runs the Client App on their own computer, and it either
installs a server there or connects to a server somebody else runs. A
client-only application cannot show the server's Product App before a server
exists, so the local path needs one bootstrap path and one trust root.
Remote-development products separate the client from the runtime where work
happens: the Client App owns setup and the lifetime of the process it starts,
and the Server Runtime owns the Workspaces and the Product App.

The Client App is Electron, packaged with electron-builder, because a system
web view has a double microphone prompt and a broken display picker. On Linux,
Electron products ship a deb or rpm from a signed repository, an AppImage, or
both; Signal and 1Password sign theirs with a detached OpenPGP signature. No
Linux distribution has a notary checked before first launch.

## Decision

### Two installation methods

- **The Headless Server**, a Linux container image for a team on a VM
  (ADR-0024).
- **The Client App**, on macOS arm64 and Linux amd64 and arm64. Setup asks
  whether to install on this computer or to connect to a server. Install
  installs and supervises one exact Server Runtime release and asks "Just me"
  or "Several people". Connect opens the server's own sign-in page, or the
  page of a Sign-In Link of the server (ADR-0028).

Nothing converts one installation into another. Local is not offline: a local
installation keeps Workspaces, memory and Computers on the machine and sends
model requests to the configured providers, so no interface claims that
content stays on the device.

### The Client Credential

A local installation writes a Client Credential under the workspace home,
readable by the machine's owner alone. The Client App trades it for a Session
of the seeded Administrator, so the person at the machine does not sign in
again. A browser never receives it; a browser signs in with a password or a
one-time sign-in link that the daemon prints at start, good for one minute and
one use, the one daemon URL that carries a secret. The link starts at the
daemon's loopback origin, not the Public Origin, so a local installation binds
loopback or every interface.

`--local` decides whether an installation holds the credential. The Client App
starts the Server Runtime with it, as does a developer who runs from source;
the headless image never passes it and refuses to start with it (ADR-0024). A
local installation holds a credential whatever its Public Origin. The default
is a server, so a start that forgets the flag asks for a password. The flag
decides and not the configuration, because a local installation in Remote
Access and a server behind a same-host proxy have the same configuration. The boot
records the answer once, and every other site reads the record. A server boot
removes a credential file left in the workspace home, and an installation with
no credential refuses the trade and the link.

The trade, the sign-in link and the runtime identity handshake answer only a
request from a program on this machine that did not come through a proxy.
`pagis_server::forwarded::is_from_this_machine` holds the rule:

1. The socket peer is loopback. The daemon reads the socket, never
   `X-Forwarded-For`.
2. The request carries no proxy header: `Forwarded`, `Via`, `X-Forwarded-For`,
   `X-Forwarded-Host`, `X-Forwarded-Proto` or `X-Real-IP`.
3. The `Host` header names `localhost` or a loopback address.

Any other request gets `403`. A same-machine proxy connects from `127.0.0.1`
like the Client App, so the Trusted Proxy address cannot be the test; such a
proxy fails rule 2 or 3, named as the Trusted Proxy or not. The documented
proxies write `X-Forwarded-For`, which the rate limit also needs. The same rule
decides where a `byo` Google Connection starts (ADR-0012).

### The Runtime Lock is the trust root of a download

Wherever the Client App installs the Server Runtime, the rule is one release,
one platform, one package, one Computer image, and no compatibility adapter.
The signed Client App embeds the Runtime Lock of its platform as
`runtime-lock.json`. The release publishes `runtime-lock-darwin-arm64.json`,
`runtime-lock-linux-x64.json` and `runtime-lock-linux-arm64.json`.

```json
{
  "schema": 1,
  "release": "<semver>",
  "platform": "darwin",
  "arch": "arm64",
  "asset": {
    "format": "dmg",
    "name": "pagis-server-<release>-aarch64-apple-darwin.dmg",
    "url": "<immutable release asset URL>",
    "size": 123,
    "sha256": "<64 lowercase hexadecimal characters>",
    "team_id": "<team identifier read from the signing certificate>"
  },
  "entries": [
    { "path": "pagis", "kind": "executable", "codesign_id": "com.pagis.server", "size": 123, "sha256": "<sha256>", "mode": 493 }
  ],
  "computer_image": "<image>@sha256:<64 lowercase hexadecimal characters>"
}
```

The Linux lock has no `team_id` and no `codesign_id`, because Linux has no
platform code signature; `arch` is `x64` or `arm64`, and the asset is a
`tar.gz` named `pagis-server-<release>-<rust target>.tar.gz`. Platform and
architecture names are Node's, compared with the client's own process.

The lock records the byte counts and hashes of the finished artifacts, and on
macOS the team identifier of the actual signing certificate. Every field of
the platform's shape is required, and a field of the other shape is refused.
The entries are the same five regular files at the root on every platform,
with unique paths. The client refuses a lock of another platform,
architecture, schema, release, format, asset name, team, size, hash, entry,
mode, code identifier or image digest. It never reads a mutable release
catalog, a "latest" URL, a checksum beside the download or a version range.
The client, the server package and the Computer image are one release tuple.
The lock is generated after the server package and the image are immutable,
and the client build checks that its version and tag equal the release, then
embeds the lock before the client or its checksum is signed.

### The server package on each platform

On macOS the server asset is a read-only disk image. Both executables are
signed with the release's distribution identity, a hardened runtime and stable
identifiers, and their designated requirements bind those identifiers to the
certificate's team. The image is signed, notarized and stapled; a ZIP or a bare
binary cannot carry a stapled ticket, and the system tools build and verify a
disk image with no custom cryptography.

On Linux the lock is the whole check of the server package, a gzip tar
archive for the client's architecture holding exactly the five files. It is
not an installation method; nobody installs it by hand, and there is no archive
for macOS. The Linux Client App is an AppImage and a deb for amd64 and arm64.
The release publishes `Pagis-<release>-linux.SHA256SUMS`, the SHA-256 of the
four packages, and `Pagis-<release>-linux.SHA256SUMS.asc`, a detached OpenPGP
signature by the Pagis release key, whose public key is `docs/release-key.asc`
at the release tag.

The package holds exactly the lock's entries, and the server binary embeds the
Product App. A bundled third-party binary keeps its license and a Pagis notice
beside it; the release checks its upstream hash before signing or archiving,
and the lock records the hash of the distributed copy. Pagis ships no Git,
Docker, Node, Python or plugin runtime. Git is a feature prerequisite: the
daemon uses libgit2 for memory, and the `git` command runs repacks and
installs git Plugins; without it, chat and uploaded Plugins work and a git
install reports that git is missing. Docker is optional and belongs to the
Computer setup.

### Package files and workspace files never mix

The Client App installs releases under its own application data directory: a
downloads directory, a staging directory, one immutable directory for each
release named for its release, platform and architecture (`0.3.0/linux-x64`),
an active-release file with only the release, platform and architecture, and a
launch marker. The installation's data stays under the workspace home,
`~/.pagis` by default. Removing or repairing a package never touches that data,
the secret file, the Client Credential, the Installation Key in any of its
places, Computer volumes or logs. Client storage may hold package progress and
the last package error, never provider credentials or onboarding answers. The
client keeps no copy of the Client Credential, and its Session lives in the
product window's cookie store, which the setup page cannot read.

The client is single-instance and serializes download, install, activation and
repair. It downloads to a partial file, checks the size and hash, opens the
package in a fresh staging directory, and copies out only the named regular
files, refusing links, devices, extra files, paths outside the root, wrong
modes and any size or hash mismatch. On macOS it mounts the image read-only,
keeps the download quarantine, runs a strict signature check on both
executables, requires each designated requirement to match the lock's
identifier and team, and requires the copies to carry the quarantine. On Linux
it checks the archive against the lock, requires its member list to equal the
five names, extracts them with the system `tar`, and requires each file to be a
regular file with one link and the locked mode, size and hash. It renames the
staged directory into place on the same volume. A failed check runs no code,
an interrupted install leaves the active release and the data untouched, and a
cache is reused only while its bytes match the lock.

### The Client App owns the server process it starts

The client writes the launch marker atomically with the release to try and the
prior active release, then starts the server with the workspace home, the flag
that suppresses the server's own first-run browser, and `--local`, on the
configured loopback port. It waits for health, reads the Client Credential and
asks for the runtime identity with a new challenge. The proof is keyed by the
credential, so a process without the file cannot answer, and the client
requires the lock's server identity and version. It trades the credential for a
Session, replaces the active-release file atomically, removes the marker, and
opens the Product App at the first unanswered onboarding step.

The client records the process it starts and stops only that process on quit.
It may attach to a healthy server only where the identity proof names the same
installation and release, and never kills a process for holding the port.
Closing the window keeps the application in the tray with the server running,
because Schedules and Wake-ups need it. The tray item is in the macOS menu bar
and in the Linux status area through StatusNotifierItem; every action is in its
menu, because a Linux status area sends no click. With no status area there is
no item, and the launcher brings the running window forward. "Open at login" is
a login item on macOS and an XDG autostart entry on Linux. The menu opens the
signed-in Administration Interface, and the Client App registers as the Host of
its machine (ADR-0015).

Setup is a local, client-owned page shown when no active server is usable: a
short flow in a fixed-size window as high as its tallest regular screen, with
the content centred between the title bar and a footer that holds Quit on the
left and the next step on the right. A long state scrolls in the middle area
alone. The first screen asks "How do you want to use Pagis?" with "Install on
this computer" and "Connect to a Pagis server"; the second choice shows the
Server address field, which also takes a Sign-In Link, and, below it, the Host
trust in one line (ADR-0015). After
"Install on this computer" a second screen asks "Just me" or "Several people"
with Back and Install. The window then shows progress, a taken port, or a
failure with Repair, Cancel and "Choose another setup". Quit asks first only
while an installation or a start-up runs. Both answers run the same local
installation; "Several people" then opens the Administration Interface on
`/settings#remote-access`, where the owner turns on Remote Access (ADR-0028).
The answer is stored nowhere.

Privileged setup IPC accepts a call only from the exact setup view, from its
main frame, at the packaged setup URL, and validates every argument. The
product window has no node integration, context isolation and renderer
sandboxing on, no installer preload and no native bridge. Its navigation allows
only the loopback origin of the server this client started or the origin of
the server it connected to, and approved external links.

A taken port is the one failure the server cannot report through its own
interface. The client names the holding process with `lsof` on macOS or `ss`
on Linux, else the port alone, proposes the next free port, writes the config
and starts again on accept. There is no random port. The proposal is never the
Administration Port. A taken Administration Port has its own server message;
the client names its holder and proposes no port, because `[administration]
port` moves it. An exit with the reserved restart code (ADR-0024) makes the
client start the server again.

### Local onboarding

The Product App opens the local onboarding: welcome, providers, computer. It
sets the provider keys, the default model and the Docker endpoint through
onboarding routes on the product port that answer `409` once onboarding is
finished (ADR-0024). The daemon records the progress, so a reload or restart
opens on the first unanswered step.

The providers step takes a key for each provider the person has, in one form.
Each provider says what its key does in Pagis: thinking, spoken replies,
dictation, calls. A
summary says what the keys cover and names the key that each missing part
needs, so a person with an Anthropic key alone learns that calls need an
OpenAI key before a call fails. A stored key of a provider that thinks is
enough to finish. The optional check of each key reads the provider's model
list with the key, which costs nothing, and passes when the provider answers,
with no comparison against a model name Pagis holds. A typed key is stored
only when the check passes. A key the provider refuses (401 or 403) fails
with the provider's words, is not stored, and does not replace the
installation's key. Only a passed check reads as ready: "the key works; N
models available". The daemon keeps one check for each provider.

The step offers one model picker over the lists of the keyed providers, each
newest first: Anthropic lists newest first, and the daemon orders an
OpenAI-shaped list by `created`, as Open WebUI, LibreChat and Continue do. The
selection follows the Model Preference of the `default` alias, which names one
model for each provider, best first: `openai/gpt-6-luna`,
`openrouter/openai/gpt-6-luna`, then `anthropic/claude-sonnet-5-5`. The step
selects the first preferred model that a keyed provider lists, else the newest
listed chat model. The newest model of a list is not a good default by itself:
OpenRouter lists small free models first. Cline works the same way: its OpenRouter picker offers the live list
and starts on one default model that the program names for each provider. The
picked model is the whole `default` alias. Without a list the daemon names the
provider's preferred model. A Person an Administrator creates takes the
Administrator's route less every candidate whose provider has no key. A
Workspace nobody picks for takes the first preferred model whose provider has
a key and lists it. Else it takes the newest listed chat model of the first
keyed provider, in the order of the preference, and else that provider's
preferred model. The environment setup runs before any list exists, so it
takes the first preferred model whose provider the environment gives a key.
The seed names the first preferred model. A key refused at a Run gives a
conversation message that an Administrator sets up providers under Providers
in the Administration Interface. The seed never adds a candidate on another
provider, because a silent fallback changes the provider, the price and the
tools; fallback candidates are the person's choice in Settings under Models.

The computer step pings every known container endpoint and reports each
answer. Once the person asks for a Computer, the daemon owns the pull through
the normal Computer lifecycle, so a second click joins the job and leaving the
page does not stop it. Finished onboarding and a ready Computer are separate
states.

### A connected client holds a Compatibility Range

A client that connects to a server downloads no server and owns no server
version, so it holds a Compatibility Range: the SemVer range of its own release, its version
and every later version that promises the same API. It reads the version from
the health route at every start and refuses a server outside the range with a
message that says which end to update. Trust rests on TLS and the sign-in.

Setup asks for the server address or a Sign-In Link of the server
(ADR-0028), and nothing else. The client checks the origin of either with the
rules of `serverOrigin`, reaches the health route and applies the range; a
server not yet set up says so and names the Administration Interface. A link
with no secret is refused. Problems show under the field. When the checks
pass, the product window opens at the server's origin, and the person signs in
on the server's own page, as the Slack, Mattermost and Element clients do; the
client never holds the password. For a link, the product window opens at the
link, which the client makes again from the checked origin and the secret. The
page of the link trades the secret for the Session in the product window, so
the client sends the secret nowhere itself. `server.json` holds the origin
alone, and the secret goes to no file, no client storage and no log. When the
product window's cookie jar holds a Session of the server, the client registers
the machine as a Host, and on `https://` holds the cookie as `Secure`
(ADR-0024). It has the same tray lifetime and supervises no process.

### Updates never roll data backwards

The Client App installs Updates, and a newer Client App upgrades its Local
Installation at start (ADR-0027). It installs its own tuple beside the active
release. A failure before launch leaves the old release usable;
once the marker names the new release, every restart stays on it, because
opening the installation may have changed its data, and the client offers
retry, repair or a newer client.

The server owns a monotonic release marker under the workspace home. Before it
opens the database, a server refuses to run when its release is older than the
marker or the marker is invalid; a newer server writes its release atomically
first. Clearing client state cannot lower it. There is no database rollback. An
inactive package directory is removed only when no process owns it, and data
is never part of that cleanup.

### A release is four artifacts

| | Artifact | Platforms | Number |
| --- | --- | --- | --- |
| 1 | The Client App and its installer | macOS arm64 (DMG); Linux amd64 and arm64 (AppImage, deb) | The release |
| 2 | The Computer image | linux/amd64, linux/arm64 | Its own image version |
| 3 | The server package | macOS arm64 (disk image); Linux amd64 and arm64 (gzip tar) | The release |
| 4 | The Headless Server image | linux/amd64, linux/arm64 | The release |

Artifacts 1, 3 and 4 come from one commit with one number, and are never
published apart. The Computer image changes on its own schedule, so a release
pins one version resolved to an immutable digest, and a published version of
it is never pushed again. A release publishes these four, the three Runtime
Locks, the signed Linux checksum list, and the Update files of ADR-0027, and
nothing else.
`docs/RELEASING-SERVER.md` states the matrix.

A `v*` tag that names the workspace version builds all four in one workflow
run, after the gate passes on the tagged commit. Each stage runs on the host
it needs: each architecture of each image on a Linux runner of that
architecture, the Linux server packages on Linux, the signed macOS server
package on macOS. The run holds the server packages and the
locks in a draft GitHub Release, and builds the clients from those locks. The
draft becomes public only when a maintainer approves the publication. The run
attests the provenance of each image digest and each package it builds, so
each artifact carries a Sigstore signature of the commit and the workflow that
built it.

The Computer image is pushed and resolved to its digest first. The Headless
Server image is built against that digest before anything is signed, so a
release that cannot make it stops early. Each Runtime Lock is written from the
final bytes of its server package. A client is published only when every asset
it references is present and matches its lock, and a Linux publication signs
the checksum list with the release key and verifies it against
`docs/release-key.asc` first. `docs/RELEASING-SERVER.md` states the steps.

Deterministic tests cover the lock parsers, both package installers, the
supervisor against a fake server binary and identity proof, the credential
trade and its refusals, the setup IPC, the upgrade markers under a crash and
cleared client state, the release order, and backup and restore on both
Storage Backends. The release jobs check the exact signed packages: the
signatures, the notarization tickets, Gatekeeper, the embedded lock and a
smoke launch of the client.

## Consequences

- The server is the product, and the Client App is an installer and supervisor
  whose signed bytes are a trust root: a notarized app on macOS, packages under
  a signed checksum list on Linux.
- A Linux release needs the release key as a secret of the protected
  `release` environment. The key has no passphrase, because the publication
  job signs with no person present.
- A release needs a Developer ID Application certificate and an App Store
  Connect API key as secrets of the repository. No person's Apple ID takes
  part.
- A restart exit code, a system-settings endpoint, a no-browser flag,
  `--local`, a release marker and the local credential trade are part of the
  server's contract.
- A connected client keeps a Compatibility Range, and the exact-tuple rule
  stays on the download: two trust problems, two rules.

## Not built

- A signed apt or rpm repository. A Linux person installs each release by
  hand, and the deb carries no package signature.
- The AppImage on Ubuntu 24.04 and later: Ubuntu refuses unprivileged user
  namespaces to a program with no AppArmor profile, and the Chromium sandbox
  needs them, so a person there uses the deb.
- An rpm. A Fedora or openSUSE person uses the AppImage.
