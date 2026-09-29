# Pagis

A staff of AI Agents that work for you, each with a job, a memory and a
computer of its own.

[![CI](https://github.com/pagis-co/pagis/actions/workflows/ci.yml/badge.svg)](https://github.com/pagis-co/pagis/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Pagis gives you a staff of Agents: AI helpers that work as your virtual
assistants. Each Agent has a job, a personality, its own memory and its own
computer, a container with a browser and a terminal. You work with them in a
chat app, in direct messages, group channels and threads. The interface
calls an Agent a sprite. One Rust daemon serves one person on a laptop or a
team on a server. It sends model requests to the providers that you
configure (Anthropic, OpenAI or OpenRouter), and it runs no model itself.

## Features

- **Agents that remember.** Each Agent keeps its memory as Markdown files
  under git. The Learning Feed shows each memory change, with a revert.
- **A computer for each Agent.** You watch its screen live, take control at
  any time and give control back.
- **Connections with scoped access.** Mail and calendar through Google, a
  Grant for each Agent, approvals for actions that change the world, and a
  Vault whose secrets the model never sees.
- **Proactive work.** Schedules and Event Subscriptions start work while you
  are away.
- **A phone number and a mailbox.** An Agent can hold its own number, make
  and answer calls, and send and receive mail from its own address.
- **Software and Plugins.** Agents build versioned tools that other Agents
  use and fork. An Administrator installs Plugins: Skills and MCP servers.
- **Hosts.** An Agent runs an approved command on your own machine through
  the Client App.
- **One person or a team.** One installation is one Org. Each Person has a
  private Workspace, and an Administrator manages the People, the spend and
  the providers.

## Quick start

A release installs in two ways:

- **The Client App** (macOS and Linux) runs Pagis on your own computer or
  connects to a server. The [Quickstart](https://docs.pagis.co/quickstart)
  installs it.
- **The Headless Server** (Linux VM with Docker) runs Pagis for a team.
  [Deploy with Compose](https://docs.pagis.co/server/compose) starts it.

### From source

You need Rust, Node.js (the version in `.nvmrc`) and, for the Agents'
computers, Docker.

```bash
(cd ui && npm ci && npm run build)
cargo run -p pagis -- --local
```

`--local` makes a local installation. At the first run, the daemon prints a
one-time sign-in link and opens the browser on it. `pagis --help` lists
every flag and command.

## Analytics

A release build sends anonymous analytics to PostHog once a day: the
release, the operating system, the kind of installation, the number of
People and Agents as ranges, and which features are in use. The events
carry a random installation ID. They hold no content, no names, no
addresses and no IP address. An Administrator turns them off in System
Settings in the Administration Interface, and `DO_NOT_TRACK=1` in the
environment of the daemon stops them too. A build from source sends
nothing. [ADR-0026](docs/adr/0026-the-daemon-sends-anonymous-analytics-from-a-release-build.md)
holds the details.

## Documentation

The user documentation is at [docs.pagis.co](https://docs.pagis.co), and
[docs-site/](docs-site) holds its source. Each release deploys the pages of
its own tree. The documents below are for the people who build Pagis.

| Topic | Document |
| --- | --- |
| The glossary of domain terms | [CONTEXT.md](CONTEXT.md) |
| Product direction and the parts that are not built | [docs/VISION.md](docs/VISION.md) |
| Architecture and decisions | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/adr/](docs/adr) |
| Build and package the Client App | [desktop/README.md](desktop/README.md) |
| What belongs on a memory page | [docs/MEMORY-PAGES.md](docs/MEMORY-PAGES.md) |
| The interface design system | [docs/UI-DESIGN.md](docs/UI-DESIGN.md) |
| The LLM router: usage, design, modalities | [docs/USAGE.md](docs/USAGE.md), [docs/DESIGN.md](docs/DESIGN.md), [docs/MODALITIES.md](docs/MODALITIES.md) |
| Release the server and the Client App | [docs/RELEASING-SERVER.md](docs/RELEASING-SERVER.md), [docs/RELEASING-CLIENT.md](docs/RELEASING-CLIENT.md) |
| Write and deploy the documentation site | [docs-site/README.md](docs-site/README.md) |

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) describes the development setup, the
checks and how a pull request merges.

## Security

Report a vulnerability through GitHub private vulnerability reporting. Do
not open a public issue for it. [SECURITY.md](SECURITY.md) holds the
policy.

## License

Pagis is licensed under the [MIT License](LICENSE). Each server package
also carries `LICENSE.gog` and `THIRD_PARTY_NOTICES` for the third-party
programs that it ships.
