# 0038: The routers build without the Native Parts

Status: accepted.

## Context

`pagis-server` builds its product and administration routers over one
`AppState`. The daemon supplies the stores and services and gives the
routers requests from its two listeners (ADR-0024). Some services need
sockets, processes, Docker or git. A process that supplies other stores
and services needs a build without those services.

The established library pattern separates a service from its listener.
The [axum testing example](https://github.com/tokio-rs/axum/blob/main/examples/testing/src/main.rs)
gives a router requests through Tower without a port.
[Cargo features](https://doc.rust-lang.org/cargo/reference/features.html)
select code and optional dependencies independently of the build target.
These patterns let the same router serve a test and another process.

## Decision

### An Embedder supplies the stores, services and requests

An **Embedder** builds the two routers over its own stores and services,
and gives them requests. A test with no port and a process on
`wasm32-unknown-unknown` are Embedders. An Embedder can also run on a
native target. It owns how requests reach the routers.

Host names a machine of a Person (ADR-0015). It does not name an Embedder.

### One Cargo feature holds every Native Part

A **Native Part** is a group of routes whose services need sockets,
processes, Docker or git. The Native Parts are:

- Computers (ADR-0014).
- Calls (ADR-0020).
- Hosts and Coding Sessions (ADR-0015, ADR-0033).
- Software, including the Software List and Widgets (ADR-0016).
- Connections (ADR-0012).
- Remote Access (ADR-0028).
- The System tab with Analytics (ADR-0024, ADR-0026).

`pagis-server` has one default Cargo feature, `native`. It holds every
Native Part. The daemon turns it on. An Embedder that does not need the
Native Parts disables default features.

The choice is which parts a build holds, not which target it builds for.
A Cargo feature makes that choice. A target `cfg` does not: a native
test Embedder can build without `native` too.

There is one feature because no Embedder chooses between Native Parts.
The issue that makes a Native Part portable also removes it from
`native`. A part does not come back. Its routes and services then build
without the feature on every target.

### Plugin routes are portable

`AppState` holds `Plugins` and `PluginHosts` in every build. The Embedder
supplies `PackageSource` and `PluginStates` to `Plugins`, and the same
`PluginStates`, `McpHttp` and `StdioServers` to `PluginHosts`. These
services hold the supplied seams. The daemon supplies git states, a git
source, HTTP requests and native stdio servers. An Embedder without
stdio uses `NoStdioServers`.

The Plugin list, detail, install, update, Binding, Grant, start, uninstall
and log routes belong to the portable routers. Their route rows have no
Native Part. The Administration Port keeps its Administrator guard for
Plugin writes. The Org owns the installs, and a Person reads their own
Workspace's log and grants a Plugin to their own Agents (ADR-0017).
Software and Widgets keep the Native Part named `Software`.

### An absent Native Part answers 501

Without `native`, each route of a Native Part stays in the route table
and answers `501 Not Implemented` with the code `not_available`.

[HTTP semantics](https://www.rfc-editor.org/rfc/rfc9110.html#name-501-not-implemented)
define `501` for functionality that the server does not support. `404`
says that the record or route does not exist. `503` says that the failure
is temporary and the caller can try again. `501` says that this server
does not do it. A build choice does not remove a route from the API.

### Frame protocols take MessageSocket; upgrades are native

The five frame protocols are functions over `MessageSocket`:

| Protocol | Function |
| --- | --- |
| Events | `ws::serve_events` |
| Dictation | `voice::serve_dictation` |
| Call listening | `calls::serve_listen` |
| Coding Sessions | `session_socket::serve_sessions` |
| Home Exit | `exit_socket::serve_exit` |

The upgrade handlers need the `ws` feature of axum, so they are native.
That [axum feature](https://docs.rs/axum/latest/axum/#feature-flags)
supplies the WebSocket extractors. An Embedder upgrades its own socket
and gives a `MessageSocket` to the protocol function. The HTTP upgrade
and the frame protocol are separate concerns. A protocol whose service
belongs to a Native Part still needs that service.

### OpenAPI describes the full build

The OpenAPI contract is the contract of the full build. A build without
`native` does not generate a smaller contract. The route table and the
API types stay the same; an absent Native Part has the defined `501`
answer.

## Consequences

- A test can give the routers requests without a listener.
- An Embedder chooses its stores, services and request transport.
- One feature controls all Native Parts. A portable part stays outside it.
- The daemon serves the full API. An Embedder can serve the portable
  parts and give the defined answer for an absent Native Part.

`pagis-server` builds without `native` on `wasm32-unknown-unknown`.
The Agent, conversation and Plugin route tests build `AppState` over
the same fake stores and services, then send requests through Tower with no
listener. They cover Channels, messages, Threads, Runs, Approvals,
the Needs-You Queue, memory, spoken messages and the Plugin lifecycle.
Person-scoped reads check the Workspace of the Session. Plugin lists
read the Org's Workspace. The test binaries run under Node and on
native targets.

`pagis-server::fake` holds the in-memory pair of `MessageSocket`s.
`pagis-testkit` uses this pair. The live event tests send the first
`auth` frame, subscribe to a Channel, read its events and check that
Session sign-out closes the socket.

The call core of `pagis-telephony` also builds on
`wasm32-unknown-unknown`. An Embedder supplies carrier frames and typed
keypad events through `FrameLeg::pair` and takes answer frames from the
same seam. SIP, the endpoint manager, the socket listener, carrier REST
clients and disk recording stay native (ADR-0020). The Call routes still
need the server's `native` feature.
