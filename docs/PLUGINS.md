# Write a Plugin for Pagis

A Plugin is a package of Skills and MCP servers in the agent-plugins.org
1.0.0 format. An Administrator installs it for the Org from a git
repository or an upload, in the Administration Interface on the
Administration Port. Each person then grants it to the Agents of their
own Workspace that may use it. Each Workspace runs the stdio servers of
the Plugin in its own Plugin Computer, and the daemon sends the requests
to its HTTP and SSE servers. This guide is the author's side: what the
package holds, what Pagis accepts, what a server reaches, and how a Skill
reaches an Agent.

Read `docs/adr/0017-a-plugin-consumes-connections.md` for the decisions
behind the rules below.

## The package

```
plugin.json          the manifest; every package has one
mcp.json             the MCP servers; a package of Skills alone has none
skills/
  <skill>/
    SKILL.md         the instruction document
    <files>          scripts and references the document points at
```

The whole package must be 50 MB or less.

## plugin.json

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  "name": "weather",
  "version": "1.2.0",
  "description": "Forecasts from the Acme weather service.",
  "extensions": {
    "pagis": {
      "config": {
        "api_key": {
          "type": "secret",
          "title": "Acme API key",
          "description": "From the Acme console, under Developers.",
          "required": true
        },
        "calendar": {
          "type": "connection",
          "title": "Calendar account",
          "provider": "google",
          "capabilities": ["calendar.read"]
        },
        "units": { "type": "string", "title": "Units" }
      },
      "tools": {
        "forecast": { "effect": "free" }
      }
    }
  }
}
```

The `name` is the tool namespace and the Skill namespace, so Pagis is
stricter than the specification: 1 to 64 ASCII letters, digits,
underscores or hyphens, and never a period. The names `core`, `ui` and
`pagis` are reserved.

Every other `extensions` namespace is ignored. Pagis reads `pagis` only.

### config fields

A config field is a value the Administrator binds at install. The five types are
`secret`, `connection`, `string`, `number` and `boolean`. A `connection`
field must name its `provider` and should name the `capabilities` it
needs; the Agent that calls the Plugin must hold a Grant on the bound
Connection with every one of them. A field with `required: true` and no
binding leaves the Plugin disabled until the Administrator binds it.
A `connection` field binds an Installation Connection of the Org. The
Grant that lets an Agent use an Installation Connection through a
Plugin is not built, so a Plugin with a bound `connection` field does
not pass dispatch (ADR-0017).

### tool effects

`tools.<name>.effect` is what the Plugin claims about one of its tools:
`free`, `outbound`, `destructive`, `purchase` or `credential_release`. Every tool the Plugin does not declare is `host`,
which asks the person before each call. A Plugin can only raise the
class, or ask the Administrator to lower it at install; it cannot lower
it alone.

## mcp.json and `${config.<field>}`

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
  "mcpServers": {
    "acme": {
      "type": "stdio",
      "command": "./bin/acme-server",
      "args": ["--units", "${config.units}"],
      "env": { "ACME_API_KEY": "${config.api_key}" },
      "cwd": "${PLUGIN_ROOT}"
    }
  }
}
```

`${config.<field>}` is the reference to a bound config field. It must
name a field the manifest declares. Where it may appear depends on the
kind of field:

- a `secret` or `connection` field goes to a stdio server as an `env`
  value, and to an HTTP server as a header value. It may never appear in
  `command`, in `args`, in `cwd` or in a URL, because those are visible
  where an environment variable and a header are not. An `env` value is
  not private to its server: every other stdio server in the same Plugin
  Computer can read it ("Reach");
- a `string`, `number` or `boolean` field may appear in any of them.

`${PLUGIN_ROOT}` and `${PLUGIN_DATA}` are the specification's own
placeholders and are not config references. `PLUGIN_ROOT` is the
read-only checkout of the installed state; `PLUGIN_DATA` is a writable
directory that survives an update.

The other rules an install applies:

- a `command` is a bare name or a `./` path to a file of the package,
  and it carries no placeholder;
- a `cwd` is `./`, `${PLUGIN_ROOT}` or `${PLUGIN_DATA}`, and it stays
  inside what it is rooted in;
- an HTTP server uses `https`, except to the loopback interface, and its
  URL carries no user information and no fragment. Plain `http` is
  accepted only to `localhost`, `127.0.0.1` and `[::1]`. `https` is
  accepted to every host, which includes private and link-local
  addresses. The install applies this rule to the URL that the package
  wrote, and each start applies it again to the URL that the bound
  values build.

## Reach

A server of a Plugin runs in one of two places, and each place reaches
different things.

A **stdio server** runs in the Plugin Computer of the Workspace, as the
unprivileged uid of the container. It reaches the disk and the network
of that container, and nothing of the daemon. On the Headless Server,
the egress policy of the Computers applies to it: it reaches the public
internet and the containers of its own Workspace, and no other address
of the VM, no link-local address and no private address outside
`PAGIS_COMPUTER_ALLOW` (`docs/DEPLOYING-A-SERVER.md`, "What a Computer
reaches"). A Local Installation has no egress policy.

Every stdio server of a Workspace runs as that same uid, whatever Plugin
it belongs to. The Plugin Computer has no uid for each Plugin, and all
its servers share one PID namespace. A stdio server can therefore:

- read the environment of every other stdio server that runs at the
  same time, with the secrets and tokens bound to it;
- read and change the `${PLUGIN_DATA}` directory of every Plugin that
  ran in that Plugin Computer;
- stop the other stdio servers, or send them another signal.

A process that a server starts can stay until the Plugin Computer
stops, because the idle shutdown of the server does not end it. Such a
process can read the environment of the servers that start after it.

An **HTTP or SSE server** is not a process in the Plugin Computer. The
daemon sends each request itself, from its own network, so the egress
policy does not apply. A package can therefore make the daemon send
requests to:

- every HTTPS host that the daemon reaches, which includes the services
  of the LAN or the VPC;
- every HTTP service on the loopback interface of the daemon. On the
  Headless Server, the daemon has the network of the VM, so this is the
  loopback of the VM. On a Local Installation, it is the loopback of the
  computer that runs the daemon.

The package sets the path, the query and the headers of each request.
The body is MCP, and the daemon reads each response only as MCP. The
daemon adds no credential of its own.

## Skills

Each immediate child of `skills/` that carries a `SKILL.md` is one
Skill. Nothing deeper is searched, so `skills/a/b/SKILL.md` is not a
Skill; it is a file of the Skill `a`.

```markdown
---
name: forecast
description: Read the forecast for a city from the Acme service.
---

# Forecast

1. Run `./scripts/forecast.py <city>`.
2. Give the result in the units the user asked for.
```

- The **directory name** is the Skill's name. A frontmatter `name` that
  is not the directory name is refused at install, because the directory
  alone names the mount and the load.
- The **description** is the frontmatter `description`, or the first line
  of the body when there is none. It is folded to one line and capped at
  200 characters. Write it so an Agent can tell from the one line
  whether to open the Skill.
- The **body** is what the Agent reads. Write it as instructions to a
  reader who has a terminal and a browser.

An Agent that holds the Plugin's Grant sees `<plugin>:<skill>` and the
description in its context, and reads the body with `skill_load`. Over
40 Skills, the first 40 by install order are listed with a line naming
how many more exist, so keep a plugin's Skill count small.

A Skill's own files ride into the Agent's Computer: each granted
Plugin's `skills/` directory is mounted read-only at
`/opt/plugins/<plugin>/skills/`, so `./scripts/forecast.py` beside the
`SKILL.md` resolves where the Agent works. The mount is read-only, and
the Agent's own notes about a Skill live in its memory, never in the
Plugin.

Skills carry no secret. The Computer holds no credentials, and a
`${config.<field>}` reference is not expanded in a Skill.

## Install, update and uninstall

The Administrator installs from a git URL with an optional ref, or
from one tar upload. Every installed state is a commit in a repository the daemon
owns. An update reads the source again, names the files that changed,
and mints the next Capability Manifest version; a Run that is already
under way keeps the state it started with. An uninstall stops the
servers, revokes the Grants in every Workspace, forgets the secrets and
deletes every directory.

A renamed plugin is another plugin: an update whose `plugin.json` names
a different plugin is refused.

The installed Plugins are not isolated from each other. The stdio
servers of a Workspace share one uid, so each one can read the secrets
and the data of the others ("Reach"). An Administrator therefore
installs together only Plugins that they trust together. The install of
an HTTP or SSE Plugin trusts its author and the operator of its remote
server with the reach of the daemon's network.

The install card in the Administration Interface shows the package
before the Administrator keeps it: every server with its command or URL, every `env` entry and every
header with the value as written, every tool with the effect class it
takes, and every Skill with its description. A tool the package
declares below `host` asks nobody at the moment of a call, so the
Administrator accepts those tools by name on the card. Write the `title` and the
`description` of each config field for that card: they are the words
the Administrator reads while the plugin waits for the binding.
