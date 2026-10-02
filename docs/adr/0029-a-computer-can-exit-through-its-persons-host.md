# 0029: A Computer on a server can exit to the internet through its Person's Host

Status: accepted.

## Context

A Server runs on a VM in a data center, so every Computer on it reaches the
internet from a data-center address. Sites treat such an address as a bot:
they show CAPTCHAs, refuse sign-ups, block Google and LinkedIn pages, and
mark mail as spam. A Local Installation does not have the problem, because
its Computers run on the owner's machine and leave from the owner's home
connection.

Cloud-browser products sell residential proxies or take the customer's own
proxy: Browserbase, Steel, Anchor and Kernel. Agent products that need the
person's own address move the browser to the person's machine: Manus
Browser Operator and Claude in Chrome run in the person's own Chrome. No
product found sends a cloud browser out through the person's own machine.
Tailscale exit nodes are the model of a person who sends their own traffic
through their own home connection, with consent.

A Pagis Server already holds an outbound, authenticated socket from the
Client App on each Host (ADR-0015). The same machine can carry the
Computer's traffic, with no port open at home.

## Decision

### Every Computer sends its connections to the Exit Proxy inside it

The **Exit Proxy** is an HTTP proxy on loopback inside every Computer,
which screend runs. Chromium starts with `--proxy-server` set to it, and
every shell has `HTTP_PROXY` and `HTTPS_PROXY` set to it, with `NO_PROXY`
for loopback and the Docker host. These settings never change, so a
Computer never restarts for the Home Exit. Chromium's policy sets
`QuicAllowed` to false, so its pages use HTTP/2 or HTTP/1.1 over TCP and
pass through the proxy.

A client sends `CONNECT` for `https://`, `wss://` and `ws://`, and the
proxy answers with a TCP tunnel. A client sends each plain `http://`
request in absolute form, and the proxy forwards it to its host as an
HTTP/1.1 request. Chromium sends requests to several hosts on one
connection to the proxy, and curl sends `http://` the same way. Every
connection that the proxy opens, for a tunnel or for a forwarded request,
goes through one dial, and the mode chooses the dial. A dial carries raw
TCP to `host:port`, so the HTTP stays in the Exit Proxy, and the daemon
and the Client App carry bytes and nothing else.

The Exit Proxy has two modes, and the daemon switches them on a running
Computer over its control channel, where `GET /exit` reads the mode and
the connections that the proxy holds, and `POST /exit` sets the mode:

- **Direct.** The proxy dials each connection from the Computer, as a
  Computer with no proxy does. This is the mode of every Computer whose
  Person has no Home Exit on, and of every Computer of a Local
  Installation.
- **Home.** The proxy sends each connection to the daemon, which carries it
  through the Person's Home Exit when that Host is present, and from the
  server when it is absent. The proxy authenticates to the daemon with the
  Computer's own token, so the daemon knows the Computer and its Person.

At each switch the Exit Proxy closes the connections it holds, also when
the mode stays the same, so Chromium opens new ones on the new path at
once, and no page keeps one address for some requests and another for the
rest. In Home mode the proxy also writes Chromium's managed policy with
`WebRtcIPHandling` set to `disable_non_proxied_udp`, and Direct mode
removes it. Chromium watches its policy directory and applies that policy
with no restart. `QuicAllowed` takes effect only at a start, so it is
false in every mode.

A `CONNECT` and an absolute-form request carry the host name, so the name
resolves where the connection leaves, and the site sees one address for
the name and the connection. A tool that ignores the proxy leaves from the
server, which is the address that Home mode also uses when the Home Exit
is absent. The daemon refuses the private and link-local destinations
that the egress rules refuse.

### The Home Exit is one Host of the Person, chosen by that Person

The **Home Exit** is the Host through which a Person's Computers reach the
internet. A Person turns it on in Settings and chooses one of their own
Hosts that declares the `exit` capability. Only a Host of the same Person
can be the Home Exit of that Person's Computers, so an Agent never leaves
through another Person's machine. An Administrator can turn the Home Exit
off for the whole installation with a System Setting, and can never turn it
on for a Person.

The Client App opens a second authenticated WebSocket for exit traffic, so
bulk bytes never wait in front of a `host_shell` dispatch. Each `CONNECT`
is one stream on it, multiplexed with yamux. For each stream, the Client
App resolves the name, refuses a loopback, private, link-local,
carrier-grade NAT or multicast address after the lookup, dials, and copies
the bytes both ways. The check after the lookup stops a name that resolves
to the home network.

When the Home Exit is absent, a new connection leaves from the server, and
the connections that it carried close. The Computer's view and the Agent's
`computer` tool result say which exit is in use, such as "exit: MacBook
Pro" or "exit: server". A sudden change of address is a signal that
sites read, so the Person sees each change.

### The Person turns it on knowing the cost

The Settings card that turns the Home Exit on says, before the switch:

- Sites see this machine's internet address for every page the Agents open.
- A block or an abuse report lands on that address, and sites can link the
  Agents' accounts to the household's own accounts.
- Every byte that a Computer loads crosses this connection twice, once in
  and once out, and the upload speed limits the pages.
- Some internet providers forbid a proxy service in their terms.
- The Agents reach nothing on this machine or its local network.

While exit traffic flows, the tray item of the Client App says so with a
byte count, and its menu turns the Home Exit off.

Other ways were considered:

- **A Tailscale exit node** for each Tenant Network, through a sidecar
  container. Every Person would need a tailnet, the sidecar would need
  kernel networking or a SOCKS5 server, and Pagis would hold a second
  identity for each Host.
- **A separate tunnel program**, such as chisel or wstunnel. It brings a
  second authentication beside the Session that the Host already holds.
- **A full tunnel**, such as WireGuard. It carries UDP too, but the Client
  App would need root on Linux and a network extension on macOS.
- **No traffic when the Home Exit is absent.** This keeps one address for
  each session, but every Schedule that browses at night would stop while a
  laptop sleeps. A Computer keeps working from the server instead.
- **Every Computer of a Server through a proxy in the daemon.** It needs
  no proxy in the Computer, but every Computer's internet would then pass
  through the daemon and stop at each restart of the daemon, for a Home
  Exit that most People do not turn on.
- **Chromium's proxy policy and the shell environment, set at the switch.**
  Chromium applies a proxy policy with no restart, but a shell that is
  already running keeps its environment, and the connections that are open
  keep their old path.

## Consequences

- Chromium in every Computer uses no HTTP/3. In Home mode, WebRTC from
  inside the Computer fails or uses TCP.
- Chromium holds at most 32 connections to one proxy at a time, its
  default for the `MaxConnectionsPerProxy` policy, so a page that opens
  more waits for one to close.
- A tool in the terminal that ignores `HTTPS_PROXY` leaves from the server.
  apt is such a tool, because `pagis-apt` runs it under sudo, which keeps
  none of the proxy variables.
- Every page load gains the round trips between the server and the Home
  Exit, and the home upload speed caps it. The live screen does not use
  this path.
- The TCP stack that sites see is the Host's, such as macOS under a Linux
  Chromium User-Agent. Every residential proxy has the same mismatch.
- The Computer's clock comes from the Person's timezone and its language
  is the image's. They match a Home Exit in the Person's own country only
  where that language is the country's.
- A laptop that travels moves the address with it.

## Not built

Home mode is not built. Every Computer runs its Exit Proxy in Direct mode,
and nothing in the daemon switches it. The parts:

- Home mode of the Exit Proxy, its `WebRtcIPHandling` policy, the daemon's
  side of Home mode, and the egress rule that lets a Computer reach it.
- The `exit` capability, the exit WebSocket and its streams, and the
  address check in the Client App.
- The Home Exit setting, its System Setting, the exit in use in the
  Computer's view and in the tool result, and the tray count.
