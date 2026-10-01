# 0027: The Client App installs Updates, and its Local Installation upgrades with it

Status: accepted.

## Context

The Client App is the trust root and the supervisor of one exact Server
Runtime (ADR-0025). A Local Installation runs for weeks in the tray, so it
falls behind unless the Client App replaces itself and then starts the
Server Runtime of the new release.

Products that supervise a local server update the application, and the
application then restarts its server. Ollama checks every hour, downloads in
the background and shows "Restart to update"; the new application stops the
old server and starts its own. Podman Desktop runs electron-updater on
Electron and asks before it restarts. Home Assistant pulls the new image
while the old container runs, then replaces the container and removes the
old image. The Coder CLI keeps the version of the server it connects to.

electron-updater installs an Update of a signed macOS application through
Squirrel.Mac. Squirrel.Mac accepts only a bundle that satisfies the
designated requirement of the running application, and that requirement
names the team. On Linux, electron-updater replaces an AppImage file, or
installs a deb with `pkexec dpkg -i`. Its feed holds a SHA-512 for each file
and no signature.

## Decision

### An Update replaces the Client App, and an Upgrade follows

An **Update** is a newer Client App release that the Client App finds,
downloads, checks and installs over itself. An **Upgrade** is the first
start of a newer Server Runtime on the data of an installation. The new
Client App embeds the Runtime Lock of its own release (ADR-0025). So when it
starts on a Local Installation of an older release, it upgrades that
installation. The Server Package stays a separate release asset, and an
Update does not carry it.

### Check, download, prepare

The Client App uses electron-updater. It checks at start and every 24 hours
while it runs. "Check for Updates…" in the tray menu and in the application
menu checks immediately and shows the result. The Client App downloads an
Update when it finds one and asks no question.

The Client App of a Local Installation then prepares the restart:

- It reads the Runtime Lock that the release of the Update publishes, and
  downloads the Server Package that the lock names into its download cache.
  This lock is not a trust root. The new Client App checks the cached bytes
  against its own embedded lock, and uses the cache only when they match.
- It tells its daemon to pull the Computer Image that the lock names. A pull
  by digest gives exactly those bytes. The new daemon runs only the image that
  its own release pins.

When a preparation step fails, the Update continues. The new Client App
downloads the Server Package itself, and the new daemon pulls its Computer
Image.

When the Update is downloaded and checked, the tray menu and the application
menu show "Restart to Update", and the Client App sends one notification.

### The trust of an Update

- **macOS.** The release publishes a ZIP of the signed, notarized and
  stapled application beside the DMG. Squirrel.Mac installs it only when the
  new bundle satisfies the designated requirement of the running bundle.
- **Linux.** The Client App embeds the public release key
  (`docs/release-key.asc`). It installs an Update only when
  `Pagis-<release>-linux.SHA256SUMS.asc` verifies the checksum list with that
  key, and the SHA-256 of the downloaded file is the value on its line in the
  list. openpgp.js does the check. An AppImage replaces itself and asks for
  nothing. A deb installs with `pkexec dpkg -i`, and the Person types their
  password.

The feeds (`latest-mac.yml`, `latest-linux.yml`, `latest-linux-arm64.yml`)
only name files. A feed is not a trust root.

### Restart to Update

1. When Runs are in progress, the Client App says how many, and asks before
   it continues.
2. The Client App stops its daemon. Each Run in progress fails, as it does at
   every restart.
3. The Update installs, and the new Client App starts.

When the Person quits the Client App with a downloaded Update, the Update
installs. A deb is the exception: a password prompt at quit or at logout
stops the shutdown, so a deb installs only from "Restart to Update".

### The Upgrade at start

A Client App whose release is newer than the recorded release of its Local
Installation upgrades the installation, and does not show setup:

1. It takes a Backup with the server program of the old release. It writes
   the Backup beside the State Directory, then moves it to
   `<State Directory>/backups/<old release>`. A Backup leaves out the
   `backups` directory. After each start of a new release, the Client App
   keeps only the Backup of the highest release, so the installation keeps
   one. After "Continue without a Backup", the Backup of the Upgrade before
   stays.
2. It installs the Server Package of its own release from the cache or from
   a download, and starts the server. The server writes the Release Marker
   and migrates the data.
3. It activates the release, removes the package directories and the
   downloads of all other releases, and opens the Product App.

The setup window shows each step and asks no setup question. A failed Backup
stops the Upgrade before the data changes, and the window offers Retry and
"Continue without a Backup". After the new server starts, the old release does not start again
(ADR-0025), and the Client App offers Retry and Repair.

### The daemon prepares its Computer Image

At boot, when Docker answers and the pinned Computer Image is absent, the
daemon starts one pull for the installation, and each wake joins it. When
the pinned image is present, the daemon removes each other image of the
Computer Image repository that no container uses. The Headless Server does
this too.

### A connected Client App follows its server

A connected Client App refuses a server older than itself (its Compatibility
Range). So it takes only the Update to its server's release, and never an
Update past it. It reads the feed of that release, not of the latest
release. When the server's release is not newer than the Client App, there
is no Update.

### A release publishes the feeds

A release publishes, in addition to the artifacts of ADR-0025, the ZIP of
the macOS Client App with its blockmap, and the three feeds. The publication
jobs upload them with the client packages.

## Consequences

- The Client App reads a mutable feed. ADR-0025 still forbids this for the
  Server Package: the lock that the Client App embeds is the only trust root
  of a Server Package, and a prepared download is a cache that it checks.
- An Upgrade costs one copy of the data on disk for its Backup.
- The daemon owns each pull and each removal of the Computer Image. The
  Client App does not use Docker.

## Not built

- Release channels, a beta, and a staged rollout.
- A setting that turns off the check or the download.
- An Update notice on the Headless Server. An operator upgrades with Docker
  Compose.
- A command that reverses an Upgrade. A Person restores the Backup with
  `pagis restore`.
