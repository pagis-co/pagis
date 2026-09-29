# 0017: A Plugin consumes Connections and never exposes one

Status: accepted.

## Context

The plugin package format is `plugin.json`, skill directories and `mcp.json`.
It defines no install mechanism, registry, credential fields, permission model
or sandbox. Its MCP servers must not carry secrets in their environment or
headers, and every studied host supplies the secret through its own
indirection. No studied host sandboxes an MCP server: a stdio server is a
subprocess with the user's privileges. The MCP specification fixes no timeouts
and no output limit, and its tool annotations are hints.

Pagis already has one account model: the Connection, which a Workspace owns
and Agents use through Grants. A plugin for a provider needs the same account
that the platform's sync and memory hooks use.

## Decision

### A Plugin is its own record

A Plugin is not a Connection. It consumes Connections and secrets through
Plugin Bindings and exposes no Connection or credential path of its own.

- A Plugin declares config fields under a `pagis` entry of its extensions:
  `type` (`secret`, `connection`, `string`, `number`, `boolean`), a title, a
  description and whether it is required. A connection field names a provider
  and the capabilities it needs. A reference to an undeclared field fails
  validation.
- The daemon substitutes bound values into the environment, the headers and
  the URL at spawn or request time. A Connection token goes to an HTTP server
  as a header on each request and to a stdio server in its environment at
  spawn; the daemon restarts a stdio server when the token rotates.
- At dispatch the broker checks two Grants: the Plugin Grant (the code may run)
  and a Grant on the bound Connection with the declared capabilities (the
  account may be touched). Connection Grants stay the one truth for account
  access.
- A source is a git URL with an optional ref, or an uploaded directory. The
  daemon commits every installed state into a bare repository for each Plugin,
  and the installed Version is a commit. An update is an explicit
  Administrator act that shows what changed and makes a new Capability
  Manifest version; running Runs keep their snapshot. An uninstall stops the
  servers, revokes the Plugin's Grants in every Workspace, removes the skill
  mounts and deletes the repository; audit rows stand alone.
- Install validates the package against its schemas, refuses a command that is
  not a bare name or a relative path, refuses a working directory outside the
  root, and shows every environment and header value on the install card.
- MCP-native OAuth is not supported. A server that requires its own OAuth flow
  does not install.

### The Org installs a Plugin, and each Workspace runs it

An install decides which code runs on the machine, so the Administrator
installs, updates, binds, starts and uninstalls a Plugin in the Administration
Interface (ADR-0024). The Org holds one version of each Plugin: the Plugin row,
its Bindings and its checkout live in the Org's Workspace (ADR-0023). A plugin
whose provider has no Connection installs disabled until one exists. A
`connection` Binding names an Installation Connection of the Org.

Each Workspace runs the Org's Plugins in its own Plugin Computer, from the
Org's read-only checkout, with its own Grant for each Agent and its own broker
registry. One daemon holds one host for each Workspace, and an install, update
or uninstall reaches every registry without a restart.

### Plugin MCP servers run in the Plugin Computer

A process on the daemon host reaches every Workspace's rows, memory, sealed
secrets and the Docker socket, so a stdio server runs in the Workspace's
**Plugin Computer**: one container for each Workspace, of the pinned Computer
image, on the Workspace's Tenant Network, under an Agent Computer's limits. The
daemon starts the server with a Docker exec and speaks stdio over its streams,
and the Workspace's tokens are in that exec's environment alone.

No Agent drives the Plugin Computer and it has no screen. It takes no place
under the per-Workspace half of the Awake Cap, and one place under the
per-server half; a wake that meets that half is refused with "this server". It
mounts each Plugin checkout read-only, so `${PLUGIN_ROOT}` is readable, and
`${PLUGIN_DATA}` is a directory on the container's own volume. The mount set
holds every installed Plugin, so one server's start never replaces the
container under another; an install or uninstall changes the set, and the
container is replaced at its next start.

An HTTP or SSE server is an outbound request of the daemon.

- **Host.** One supervised client for each Plugin in a Workspace serves every
  Agent with the Plugin Grant, and the broker is the only caller. There is one
  supervisor for each (Workspace, plugin id, server name). Sampling,
  elicitation, resources and prompts are refused or ignored.
- **Names.** A tool is `<plugin>__<tool>`. Two servers of one plugin that
  declare the same tool fail install. Plugin names follow the Software name
  rules and may not collide with an installed namespace.
- **Manifest.** Install starts each server once, takes its tool list, and
  freezes it with the plugin commit as one Capability Manifest version. A later
  change notification or a different list changes nothing installed: the desk
  shows that the tools changed, a tool absent from the manifest is refused at
  dispatch, and accepting makes a new version through the update flow.
- **Effect class.** Every plugin tool defaults to `Host`, so it waits for a
  card, which an allow-rule builder relaxes to "always allow this tool". A
  plugin may declare a class for each tool under the `pagis` extension. A
  lower class shows on the install card for the Administrator to accept; a
  higher one applies as declared. MCP annotations are ignored.
- **Lifecycle.** Lazy start at the first dispatch, which wakes the Plugin
  Computer; a 30-second startup timeout; a 120-second call timeout that a
  server may raise to ten minutes; idle shutdown after ten minutes; a crash
  restarts at the next dispatch with backoff from one second to one minute;
  three consecutive start failures mark the plugin failed until a manual start.
  At most eight calls are in flight for each server. A stdout frame over 4 MiB
  ends the session and the pending call answers Unknown; three overruns in a
  row mark the plugin failed. A call that completes within the cap resets the
  overrun count, and a successful start does not. The exec stream holds at most
  16 chunks of each stream, so a server that writes faster than the daemon
  reads waits.
- **Log.** Stderr goes to `logs/plugins/<workspace_id>/<plugin_id>.log` in the
  State Directory, at most 1 MiB, with a marker line for dropped excess; the
  next start of a server of the plugin empties a full log. The product API
  answers a Person with the end of their own Workspace's log alone, because a
  server can write Person data or a bound value to stderr (ADR-0023). The
  Administration Interface serves no plugin log. A Backup does not hold logs.
- **Reach.** A stdio server runs as the container's unprivileged uid, so it
  reaches that Workspace's disk and network and nothing of the daemon's. On the
  Headless Server the Computer egress policy applies (ADR-0014). Its working
  directory is the plugin root or data directory in the container. Its
  environment holds the container's path, home and temporary directory, the
  declared entries and the plugin placeholders, and nothing of the daemon's.
  All stdio servers of a Workspace run as one uid in one PID namespace, with no
  `hidepid` and no uid for each Plugin, so each can read the environment of the
  others that run, read and change every Plugin's data directory, and signal
  the others. A process a server starts can outlive the server until the
  container stops.

  An HTTP or SSE server reaches what the daemon's network reaches, outside the
  egress policy. Its URL is `https`, or `http` to `localhost`, `127.0.0.1` or
  `[::1]`, checked at install and at each start; a private or link-local
  address is not refused. So a server can make the daemon send requests to any
  HTTPS host the daemon reaches, internal services included, and to HTTP
  services on the daemon's loopback (the host's loopback on the Headless
  Server). The package sets the path, query and headers, and the body is MCP. A
  plugin that needs the person's own machine is a Host action (ADR-0015).
- **Secrets.** A secret binding goes into a stdio server's environment or an
  HTTP server's request header, and nowhere else. Install refuses a secret
  reference in the arguments or the URL, and the desk never shows a bound
  value. An environment value is readable by every other stdio server of the
  same Plugin Computer.

### Plugin skills are loaded, not copied into memory

- Nothing of a Plugin enters a memory repository. The system prompt lists
  `<plugin>:<skill>` with its description for every Plugin the Agent holds a
  Grant on, and a `Free` core tool returns the skill body.
- Each granted Plugin's skills directory is mounted read-only into the Agent's
  Computer, so a skill's scripts run there and its relative paths hold. Skills
  carry no secrets.
- Skills freeze with the plugin commit. An update changes the listing and the
  mount for Runs that start afterwards.
- What the Agent learns about a skill lives in its private memory in one fact
  file for each skill, and the load tool appends that file after the body.
  There is no fork and no overlay.
- Skills reach only Agents with the Plugin Grant, all of a plugin at once. A
  plugin with skills and no servers installs enabled with no bindings.
- A listed description is capped at 200 characters. Past 40 skills, the first
  40 by install order are listed with a line that says how many more exist.

## Consequences

- One account model: a Plugin cannot become a second holder of a user's
  credentials.
- A stdio server restarts when a token rotates.
- An uninstall removes a listing and a mount, never memory.
- A Workspace that runs a Plugin runs one more container, and a server's first
  call waits for it to boot. The daemon cannot kill a running exec, so a
  server ends when its stdin closes, and the container stop ends the rest.
- A plugin's writable directory is in the Plugin Computer's volume, not in a
  backup of the state directory.
- The Plugins of a Workspace are not isolated from each other or from the
  daemon's network, so an install is a trust decision: the Administrator
  installs together only Plugins they trust together, as the People of an
  installation trust each other's Computers on a shared kernel (ADR-0014).

## Not built

- No desk page shows the plugin log. The product API serves it.
- A Plugin bound to a Connection cannot pass dispatch: the access check asks
  for a Grant on the bound Connection, a `connection` Binding names an
  Installation Connection, and no Grant route can name an Installation
  Connection. What authorizes a Plugin to use an Installation Connection is not
  decided.
