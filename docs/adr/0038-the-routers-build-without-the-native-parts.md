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
- Plugins and Software (ADR-0017, ADR-0016).
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

## Not built

The separation of the other services and routes of Computers, Calls,
Plugins and Software, and Connections into Native Parts is not built.
The `wasm32-unknown-unknown` build of `pagis-server` is not built.
