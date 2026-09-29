# desktop/

The Pagis Client App, for macOS arm64 and for Linux amd64 and arm64. On Linux
it is an AppImage and a deb package.

At setup the Client App asks whether to install Pagis on this computer or to
connect to a server. On this computer it installs the exact Server
Runtime in its signed Runtime Lock, owns the process it starts, and loads
that server's Product App. Connected to a server, it loads the Product App
of that server (ADR-0025).

The setup is a short flow of screens in a window of fixed size that you
cannot resize. The window is as high as its tallest regular screen, so no
regular screen scrolls, and the content sits in the middle between the
title bar and the footer. A longer state, such as a long failure, starts at
the top and scrolls in the middle area only. The first screen asks "How do you want to use
Pagis?": "Install on this computer" or "Connect to a Pagis server". The
footer at the bottom edge holds Quit at the left and the next step at the
right, and Quit ends the Client App from each screen.

## On this computer

- **Installs one server.** The setup page asks who uses Pagis on this
  computer: "Just me" or "Several people". For either answer it downloads the
  locked server package, checks every byte, installs it under the client's
  own application data, and opens the Product App. On macOS the package is a
  signed, notarized disk image, and the client also checks each code
  identity. On Linux the package is the gzip tar archive of the client's
  architecture, and the lock's sizes, hashes and modes are the whole check,
  because Linux has no code signature to check.
- **Opens the multi-user switch for several people.** After the Product App,
  "Several people" opens the Administration Interface, signed in with the
  Client Credential, on the Multi-User Mode switch in its Settings view. The
  owner turns the mode on there, with the Public Origin of their proxy or
  tunnel. The client stores the answer nowhere.
- **Keeps packages apart from data.** The client keeps its own files in
  its Electron `userData` directory, `~/Library/Application Support/Pagis`
  on macOS and `~/.config/Pagis` on Linux (`$XDG_CONFIG_HOME/Pagis` when
  that is set). It stores downloaded and installed Server Packages in
  `userData/runtime`. The installation's data stays under its state
  directory, `PAGIS_HOME`, which defaults to `~/.pagis`.
- **Owns the server process.** It reads the port from
  `~/.pagis/config.toml`, and proves the server's Client Credential,
  installation, release, Computer image and listening port without sending
  the credential. Then:
  - it attaches to a healthy local server only where that proof names the
    same installation and the exact release;
  - it leaves alone a proven server that it did not start, and starts the
    installed release with `--no-open --local` only when the port is free.
    `--local` makes the server a local installation, which holds a Client
    Credential;
  - it reports a taken port with the process that holds it and the next
    free port: on the setup page at the first start, and on the status page
    after that. The client finds the process itself: `lsof`
    on macOS, and `ss` on Linux, which names the processes of this account
    only. Where neither answers, the page names the port alone. The next
    free port is never the Administration Port of the installation. On
    accept, the client writes the port into the config file, which is the
    one key it writes, and starts again;
  - it reports a taken Administration Port with the process that holds it.
    It proposes no port there, because `[administration] port` of the
    config file moves that port.
- **Opens the window signed in.** The daemon writes the Client Credential of
  the installation into `~/.pagis/client-credential` on its first run. The
  client trades that credential at `POST /api/v1/sessions/client`, puts the
  Session cookie into the cookie jar of the window, and only then loads
  `http://127.0.0.1:<port>/`. The credential stays in the main process and
  never goes into a URL. This holds whatever the Public Origin names: when
  other People reach the installation through your proxy or tunnel, the
  client on this machine still opens signed in. The daemon accepts the trade
  only from this machine and never through a proxy, even a proxy on this
  machine (ADR-0025).
- **Recovers safely.** Install phases survive a crash. Retry, cancel and
  repair change only files the client owns. A repair waits until no other
  process uses the installed runtime. The setup page offers Repair only
  where this computer holds an installation, and a repair asks no setup
  question. An error that no code of the client
  catches goes to the setup page, and never to a raw error dialog.
- **Keeps the server it started alive.** A crash gives three restarts with a
  one-second pause, then the failure page with the log. Exit code 75 means
  "start me again" and does not count as a crash.
- **Stops the server it started on quit.** It sends SIGINT to the exact child
  it started, then kills that child after five seconds. A server it only
  attached to continues to run.
- **Is the Host of this machine.** The client registers as a Host, so an
  Agent can run a command here after the Person approves it (ADR-0015).

Docker is optional. Local setup, chat, memory and connected tools work
without it. An Agent needs Docker for its Computer: the browser, the screen,
the terminal and the shell. Setup can continue while a requested Computer
starts.

Repair checks and replaces only the locked Server Package. It moves the
damaged release aside, and removes it when the replacement is active, or at
the next start when the client stopped before that. It does not
remove the installation's data, the provider keys, the Installation Key (the
macOS keychain item, the Linux Secret Service item or the Key File), the
Computer volumes or the logs.

## Back up and restore

`pagis backup <directory>` copies the whole installation on this computer,
and `pagis restore <directory>` puts it back (ADR-0024). The client puts no
`pagis` command on the `PATH`. The command is the `pagis` file of the
Server Runtime that the client installed:

| Platform | The `pagis` command |
| --- | --- |
| macOS | `~/Library/Application Support/Pagis/runtime/releases/<release>/darwin-arm64/pagis` |
| Linux amd64 | `~/.config/Pagis/runtime/releases/<release>/linux-x64/pagis` |
| Linux arm64 | `~/.config/Pagis/runtime/releases/<release>/linux-arm64/pagis` |

`<release>` is a release number, such as `0.1.0`. The `releases` directory
holds one directory for each release that the client installed, and the
client starts the highest one. Use that one.

A backup needs a stopped server, and `pagis backup` refuses to run beside
a running one. Select "Quit Pagis" in the tray menu first: quit stops the
server that the client started. Then, on macOS:

```bash
releases="$HOME/Library/Application Support/Pagis/runtime/releases"
ls "$releases"
pagis="$releases/0.1.0/darwin-arm64/pagis"   # the highest release that ls shows
"$pagis" backup ~/pagis-backup
```

On Linux, set `releases="$HOME/.config/Pagis/runtime/releases"` and
`pagis="$releases/0.1.0/linux-x64/pagis"` (or `linux-arm64`). The command
reads the state directory `~/.pagis`, or the directory that `PAGIS_HOME`
names. Open Pagis again when the backup is complete.

The backup does not hold the Installation Key, the Client Credential or the
access tokens of the Computers (`computer-tokens/`). The key stays in the
keychain of this account on macOS or in the Secret Service on Linux, or in
the Key File `~/.pagis/installation-key`. Copy a Key File to a safe place
yourself: the restored installation cannot open its provider keys without
it. The backup does not hold the Computer volumes
either; Docker keeps them.

The backup has no encryption. It holds the State Directory with `pagis.db`:
the records, the memory repositories with their full history, the
Artifacts, the recordings, the screenshots, the Plugins and the Software of
every Person on this installation. Pagis seals only the secrets in it, for
example `secrets.enc`. [What Pagis encrypts](../docs/DATA-AND-PRIVACY.md#what-pagis-encrypts)
names each store. Only your OS user can read the backup, but these
permissions do not protect a copy on another disk or in a cloud store. Keep
the backup on a disk with FileVault on macOS or LUKS on Linux, or encrypt it
with your backup tool, for example restic or age. Keep a Key File apart from
the backup.

`pagis restore` writes only into an empty state directory. To restore,
quit Pagis, move the current state directory away, and restore into
`~/.pagis`:

```bash
mv ~/.pagis ~/.pagis-before-restore
"$pagis" restore ~/pagis-backup
```

With a Key File, copy it back to `~/.pagis/installation-key`, mode 600.
Then open Pagis. The client starts the restored installation and opens it
signed in. The release of the client must be the release of the backup or a
newer one.

## On Linux

- **Two packages.** The deb installs to `/opt/Pagis` with the command
  `pagis-client`, because `pagis` is the server's own command. It also
  installs an AppArmor profile, which the Chromium sandbox needs on Ubuntu
  24.04 and later. The AppImage runs from where it is saved, and on Ubuntu
  24.04 and later it needs an AppArmor profile that it does not bring, so
  use the deb there.
- **The Installation Key.** The server this client starts keeps its key in
  the desktop keyring through the Secret Service (GNOME Keyring, KWallet,
  KeePassXC). Where no Secret Service answers, the server generates the Key
  File `~/.pagis/installation-key`, mode 600, and its log says which one it
  uses (ADR-0013).
- **The tray.** The tray item shows in a status area that speaks
  StatusNotifierItem: KDE Plasma, and GNOME with the AppIndicator extension,
  which Ubuntu turns on. A click there opens the menu. On a desktop with no
  status area there is no item: open Pagis from the application launcher,
  and the running client shows its window.
- **Open at login** writes `~/.config/autostart/pagis-client.desktop`. From
  an AppImage, the entry starts the AppImage file.
- **One client.** A second start shows the window of the running client and
  quits, as on macOS.
- **Verify a download.** The release lists the SHA-256 of the four Linux
  packages in `Pagis-<release>-linux.SHA256SUMS`, signed by the Pagis release
  key in `Pagis-<release>-linux.SHA256SUMS.asc`. The public key is
  [`docs/release-key.asc`](../docs/release-key.asc):

  ```bash
  gpg --import docs/release-key.asc
  gpg --verify Pagis-0.1.0-linux.SHA256SUMS.asc Pagis-0.1.0-linux.SHA256SUMS
  sha256sum --ignore-missing -c Pagis-0.1.0-linux.SHA256SUMS
  ```

## Connected to a server

"Connect to a Pagis server" shows the Server address field, and the setup
page takes that address and nothing else: no email address and no password.
It checks the address before it sends it: an empty or malformed address gets
a message under the field, and the field takes the focus. The main process
checks the request again and refuses a request with no address in words for
a person. The client installs nothing, supervises nothing and keeps no key.
It checks that a Pagis server answers at the address and that its release is
inside the client's Compatibility Range: the client's own version and every
later version that promises the same API. When the server has no
Administrator yet, the client says so and names the Administration Interface
that finishes setup. Each of these problems shows under the field, and the
setup window stays. When the checks pass, the client keeps the origin in
`userData/server.json`, closes the setup window and opens the product window
at the server's origin. There the Product App shows its own sign-in page, and
the Person signs in on it, as in the Slack and Mattermost clients. When the
cookie jar of the product window holds a Session of the server, the client
registers this machine as a Host of the server.

The client checks the range again on every start and refuses a server outside
it, with a message that says which end to update. The next start opens the
same server, signed in while the Session of the last sign-in lasts. When that
server does not answer, or its release is outside the range, the setup page
says so and offers "Try again" and "Choose another setup". It offers no
repair, because the client installed nothing. Quit stops no process, because
the client started none.

**The client connects to a server only over `https://`.** It refuses an
`http://` address before it sends a request, unless the host is loopback
(`127.0.0.1`, `[::1]` or `localhost`), such as an SSH tunnel. The refusal
names the https setups of
[`docs/DEPLOYING-A-SERVER.md`](../docs/DEPLOYING-A-SERVER.md): Caddy,
Tailscale Serve and Cloudflare Tunnel. A `server.json` that holds an
`http://` origin of another computer fails at the start, and the setup page
shows the refusal. The client follows no redirect before the product window
opens, and uses the TLS certificate check of the platform. On an `https://`
origin, its cookie jar holds the Session cookie as `Secure`, also where the
server set it without `Secure` (ADR-0024).

**When you connect to a server, this computer becomes a Host of that server.
The server and its Administrator can then run commands on this computer, with
the same access as you. Connect only to a server whose Administrator you
trust.** Under "Connect to a Pagis server", the setup page states this in
one line under the Server address field: "Connect only to a server you
trust." The client registers as a Host of the
signed-in Person and runs every `dispatch` frame the server sends, as the OS
user who started it. The approval, the effect class, the allow rules for each
machine and the audit row all live on the server, and the client applies no
check of its own, on purpose: a second confirmation here would ask a question
the server's approval card already asked, and it would not change the trust.
So a person who takes control of the server can also run commands on this
computer (ADR-0015). The client trusts the server that TLS authenticates, and
it registers as a Host on no clear-text connection to another machine.

## Both ways

- **Stays in the tray** when the window closes (the menu bar on macOS),
  because Schedules and mail Wake-ups need the server. The tray item opens
  the window, opens the Administration Interface, holds "Open at login",
  carries the new-version line, and quits.
- **Quits at once.** Quit on the setup page, in the app menu and in the tray
  asks "Quit Pagis?" only while an installation or a start-up is in
  progress, as a sheet on the window that shows it. Else it quits with no
  question. SIGTERM, SIGINT and a logout quit with no question. Each quit
  ends the process and its helper processes.
- **The Product App** comes from the server: the Server Runtime this client
  started, or the server the Person signed in to. The product window has no
  node integration, no installer preload and no native bridge.
- **Keeps each window where it loaded.** The product window stays on the
  origin of the Product App, the administration window on the
  Administration Port, and the setup and status windows on their own page.
  The client refuses a navigation or a server redirect to another place,
  because a window has no address bar that shows the change. The product
  and administration windows send a refused `https:` or `mailto:` address
  to the system browser, as they do with a new window that a page asks
  for.
- **Grants two web permissions and denies all others.** Electron grants each
  web permission that no handler decides, and it shows no prompt. So the
  client decides each request and each check. The main frame of the product
  window at that one origin gets the microphone for dictation and clipboard
  write. The client denies the camera, notifications, devices and each other
  permission. It also denies each request from a sub-frame, from another
  origin, and from the administration, setup and status windows. The Person
  sees no prompt.
- **Gives no Bluetooth device to a page.** The Product App uses no
  Bluetooth. Each window cancels each Web Bluetooth device request, so a
  page gets no device and the Person sees no chooser. On Linux, the client
  also refuses each Bluetooth pairing.

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
npm run pack         # macOS arm64: the DMG
npm run pack:linux   # Linux amd64 and arm64: the AppImage and the deb
```

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
publication so the exact DMG can pass the clean-account launch, quarantine,
keychain, offline, Docker, installation reuse, downgrade and forward-update
checks. Record those results in `dist/distribution-proof.json`.
`--publish-existing` does not rebuild. It validates the proof against the
exact client and Runtime Lock hashes, downloads and compares the server
tuple, checks the Computer image digest, and uploads without overwrite. See
[`docs/RELEASING-CLIENT.md`](../docs/RELEASING-CLIENT.md), which also
holds the Linux release.

Signing can use `CSC_NAME` with a Developer ID Application fingerprint, or
`CSC_LINK` and `CSC_KEY_PASSWORD` for an imported certificate. Notarization
can use `APPLE_KEYCHAIN_PROFILE`, with optional `APPLE_KEYCHAIN`, or
`APPLE_ID`, `APPLE_APP_SPECIFIC_PASSWORD` and `APPLE_TEAM_ID`. A tag build
fails when neither complete route exists.

`desktop/scripts/check-package.mjs` holds the package to a size ceiling.
During setup, free disk space must cover the Client App, the locked server
package, the installed entries listed in the Runtime Lock, and, during a
repair, the damaged release that the repair replaces. The client cache can reuse a verified
package when the network is offline. It refuses a changed cache file.
