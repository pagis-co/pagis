# 0014: The Computer is pixels, and Pagis owns the stream

Status: accepted.

## Context

An Agent controls its Computer through structured automation, such as an
accessibility tree over a debug protocol, or through model-native pixel
control: screenshots and coordinates. Structured control costs less for each
step but needs a large automation layer, and the model still needs screenshots
for grounding. Pixel control works on anything, and the router already speaks
it.

The watchable screen is core to the product: a low-latency live view, a user
takeover and a clean handback. A generic remote-framebuffer tool does not meet
that bar. One installation can also serve many people, whose Computers share
one Docker host.

## Decision

### Pixels are the primary control path

The ladder is API, then MCP, then structured automation, then pixels, and
pixels are the primary path for a Computer. The software factory turns a
repeated flow into a generated tool.

A screenshot after an input shows the screen after it settles, because a model
that reads a frame in the middle of a reaction clicks again. The screenshot
waits at least 300 ms, then until two frames 200 ms apart differ in at most 0.5
percent of their pixels, and at most 3 seconds. The pixels are the signal:
a menu fires no load event, a page full of ads never reaches network idle, and
a loading tab's spinner keeps the frames changing. The browser has no
debugging port, and the DevTools pipe serves the Vault fill alone (ADR-0013),
so no page event reaches the settle.

No provider's model list says which models take its own computer tool, so the
API decides. Every OpenAI model gets OpenAI's tool, and a model that answers a
400 on `tools` gets the portable `computer` function for the life of the
process. A model of another vendor behind OpenRouter gets the portable
function. It has one shape for each action, as OpenAI's tool has: each shape
requires all its fields and allows no others, and an action that does nothing
is an error returned to the model.

### Pagis owns the streaming pipeline, not the compositor

Pagis builds the streaming pipeline in Rust: compositor frame capture,
damage-aware encoding, WebRTC transport, the input channel and two-way control
handback, against a commodity headless compositor in the container. Pagis owns
the latency and the takeover end to end, and another compositor can replace the
current one behind the same pipeline.

### The container is a boundary between Workspaces

The container is the sandbox of one Agent and the boundary between
Workspaces. Five things make it one.

- **A Tenant Network.** Every container of a Workspace joins that Workspace's
  Docker network and no other. The daemon creates it at the Workspace's first
  wake and keeps it for the life of the Workspace, because a Computer stops at
  each idle eviction and a network made and removed at each wake races with the
  next wake. Each network takes one subnet from the Docker daemon's address
  pools, which set how many Workspaces one host holds.
- **An authenticated control port.** The daemon makes a secret for each
  container at each start, gives it as a read-only mount that the entrypoint
  hands to the `screen` user alone, and sends it on every control request. A
  container that reaches another's port cannot drive it, and nothing on the
  network gets around the daemon's hold of the input switch. Only `/healthz`
  answers without the secret. A Computer is awake when `/healthz` answers and
  the window list, read with the secret, holds the browser.
- **Limits and an owner on every Docker object.** A container carries memory, a
  CPU share, a process count, open-file limits, a sized `/dev/shm`, and a volume
  quota and writable-layer size where the storage driver takes them. Its name
  and labels carry the Workspace and the Agent, so a Workspace's containers and
  volumes can be listed, measured, billed and reaped, and the disk figure a
  person reads counts their own volumes.
- **Plugin servers in a container.** A Plugin's stdio MCP server runs in the
  Workspace's Plugin Computer, never on the daemon host, whose process reaches
  every Workspace (ADR-0017). An HTTP or SSE server is a request of the daemon,
  and the egress policy does not apply to it.
- **An egress policy on the Docker host.** A Computer reaches the public
  internet and the Media Relay's UDP range on the Docker host, and no other
  host address, link-local address or private address except those an
  Administrator allows. Rules in the host's DOCKER-USER and INPUT chains enforce
  it, so root in the container cannot remove them. The Headless Server
  deployment installs them. A Local Installation does not, and its
  documentation says what a Computer then reaches.

The secret and the Plugin checkouts reach a container as bind mounts of files
in the daemon's state directory, and Docker reads a bind source on the Docker
host. A daemon in a container therefore has its state directory at the same
path on the host and in its own container: the compose deployment mounts
`/var/lib/pagis` at `/var/lib/pagis`.

This is a boundary between colleagues, not strangers. The kernel is shared,
the seccomp profile allows Chromium's user-namespace calls, and there is no
virtual machine for each Workspace.

### The media path is a relay behind one seam

A browser reaches a screen through a Media Relay, never through a container.
`MediaRelay` takes the media endpoint of one Computer and returns the address
the browser sends media to and the ICE servers it configures. The screen
handler and the signalling code call that seam alone. No Computer publishes a
media port on the host.

Two implementations pass one test suite. The `daemon` relay is the default:
the daemon holds one advertised address and one UDP port range, opens one
socket for each viewer session, and copies packets between the browser and the
Computer. One socket for each session is required, because the pipeline tells
DTLS and RTP sessions apart by source address. The `turn` relay puts coturn in
front of the browser leg, with a credential for each session under the TURN
REST API scheme, so no account exists on that server and a leaked credential
expires. It suits a deployment whose firewall or scale rules out a daemon port
range. It keeps the daemon's forwarder on the container leg, because the
pipeline is ice-lite and answers one advertised candidate.

The browser leg uses ICE short-term credentials (RFC 8445 section 7.2.2). The
daemon gives each path the `a=ice-ufrag` and `a=ice-pwd` of the pipeline's SDP
answer. Media goes only to an address that sent a check with valid
MESSAGE-INTEGRITY, verified with the STUN code of str0m, and media from any
other address is dropped. This check, not the bind address, keeps strangers
out, so the relay listens on every interface.

A browser can reach the relay from several addresses at once. An answer to an
ICE check goes to the address that sent that check, by STUN transaction id.
Media goes to the selected pair: the address of the last valid check with
USE-CANDIDATE, or the last of the checked addresses that sent media, whichever
is later. Sending to whichever address spoke last starves the selected pair of
consent answers.

Each datagram leaves the relay from the address of this machine that its
receiver sends to (IP_PKTINFO). The relay socket listens on every address, and
a browser on the same machine checks the loopback candidate from an interface
address. Without the source address, the reply comes from an address the
browser never checked, and the browser discards it.

The pipeline registers outbound: for each session the relay opens one port and
makes one token, the daemon gives both to the pipeline with the offer over the
control port, and the pipeline sends the token from its media socket and
repeats it while the session lives. The relay takes the datagram's source as
the Computer's address, and the Docker host's NAT carries replies back. This
works on every platform, because a Docker host routes a container's outbound
traffic to the host even where it routes nothing inward (macOS). The container
reaches the daemon as `host.docker.internal`, mapped to the Docker host's
gateway at creation. A datagram with another token is ignored. The egress
policy keeps the relay range open from the Tenant Networks, and a local
installation whose firewall refuses inbound UDP from the Docker bridges shows
no screen.

A local installation configures nothing: the `daemon` relay advertises
loopback. The Client App gives its product window the WebRTC IP handling policy
`default_public_and_private_interfaces`, because Electron otherwise binds one
socket to each interface, and on macOS a socket bound to one interface does not
reach `127.0.0.1`. The relay keeps one advertised address, and a local
installation does not offer its LAN addresses.

### A Computer is not a Host

A Computer is Pagis's own machine, and its container is the sandbox. A Host is
the person's own machine, which Pagis does not sandbox, and there the approval
takes the container's place (ADR-0015).

## Consequences

- Computers need no automation layer, and each step costs more tokens than a
  structured rung.
- A Computer costs a control secret and a set of limits at each start, and a
  Workspace's Computers are bounded by the Awake Cap and by idle eviction.
- Docker's default address pools hold about thirty bridge networks, so a server
  with more Workspaces sets larger pools.
- The daemon removes no Tenant Network, so a test that uses the real runtime
  removes its own containers, volumes and networks. Every object a test runtime
  creates carries the `org.pagis.test` label, the test removes those objects
  when it ends, also on failure, and the test tooling removes the objects of a
  test process that is not running.
- Under the `daemon` relay, media passes through the daemon, and the UDP range
  bounds how many people watch at once. A range that runs out refuses the next
  viewer with a sentence that names the setting. Each Workspace holds one media
  path for each awake Computer: a new offer replaces the old path, a refused
  offer frees its port at once, and the path closes when the Computer sleeps.
- A closed tab tells the daemon nothing, so a path frees its port after it
  falls silent. Only authenticated ICE consent checks (RFC 7675) and the
  registration with the right token hold the port, as only an authenticated
  Refresh holds a TURN allocation (RFC 8656).
- A Docker host with no project quotas gives no volume quota, and a storage
  driver without the size option (or the containerd image store) gives no
  writable-layer bound. The daemon logs this once, boots the container, and the
  Health view of the Administration Interface reports it. The daemon learns the
  writable-layer answer from a wake's container create and from `docker info`,
  and makes no probe container, so Health reports `unknown` until a Computer
  wakes.
