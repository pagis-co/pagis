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
  Installation. The proxy resolves the name itself and opens no
  connection to this Computer itself, by loopback or by any of its
  addresses, to an unspecified address or to a link-local address
  (169.254.0.0/16, fe80::/10). These are Squid's default `to_localhost`
  and `to_linklocal` rules, with every address of the Computer added to
  loopback. A UDP bind to an address succeeds only for an address of the
  Computer, and a bind that fails for another reason than
  `EADDRNOTAVAIL` refuses the address. The proxy checks an IPv4-mapped
  IPv6 address as its IPv4 address, connects to the other addresses of
  the name in order, and answers 403 when no other address is left. The
  private address of another machine passes, and the egress rules hold
  it.
- **Home.** The proxy sends each connection to the exit listener of the
  daemon, which carries it through the Person's Home Exit when that Host
  is present, and from the server when it is absent. For each connection
  the proxy sends one `CONNECT host:port` with
  `Proxy-Authorization: Bearer <token>`, where the token is the
  Computer's control token, so the daemon knows the Computer and its
  Person. On a 200 the dial gives the stream to the tunnel or the
  forward that asked for it, and every other answer is a 403 or a 502 of
  the proxy. Every name goes to the daemon, so the name resolves where
  the connection leaves. A literal private address (RFC 1918, a unique
  local IPv6 address or carrier-grade NAT) still leaves from the
  Computer: it is on the server's network and never at the Person's
  home, so the egress rules and `PAGIS_COMPUTER_ALLOW` hold it. The
  refusal of this Computer, loopback and link-local applies to a literal
  first, as in Direct mode.

At each switch the Exit Proxy closes the connections it holds, also when
the mode stays the same, so Chromium opens new ones on the new path at
once, and no page keeps one address for some requests and another for the
rest. Each mode writes Chromium's managed policy file
`/etc/chromium/policies/managed/pagis-exit.json`: Home mode sets
`WebRtcIPHandling` to `disable_non_proxied_udp` in it, and Direct mode
writes `{}`. The image makes the file with `{}`, owned by `screen` with
mode 644, and the directory stays root's: screend rewrites the contents
of that one file and never makes or removes a file there, so no other
policy enters the directory. Chromium watches its policy directory and
applies the change a few seconds later with no restart. `QuicAllowed`
takes effect only at a start, so it is false in every mode.

A Computer takes its first mode at its wake. The daemon names the exit
listener and the first mode in `PAGIS_EXIT_DAEMON` and `PAGIS_EXIT_MODE`
of the container, and screend starts the proxy in that mode and writes
its policy file before the browser starts. An Agent's Computer on a
Server names the exit listener in both modes, so a switch to Home mode
reaches a Computer that woke in Direct mode. It starts in Home mode when
its Person has chosen a Home Exit and the Home Exit System Setting is on,
and in Direct mode otherwise. A Computer of a Local Installation names no
exit listener and runs in Direct mode alone, and the daemon of a Local
Installation opens no exit listener: its Computers already leave from the
owner's connection. The Plugin Computer names no exit listener either and
stays in Direct mode: it serves the Plugins of the Workspace, which call
APIs and not sites that score addresses.

A `CONNECT` and an absolute-form request carry the host name, so the name
resolves where the connection leaves, and the site sees one address for
the name and the connection. A tool that ignores the proxy leaves from the
server, which is the address that Home mode also uses when the Home Exit
is absent.

### The exit listener carries each connection of Home mode

The **exit listener** of the daemon takes the `CONNECT` of each Computer
in Home mode, on a Server alone, on TCP port 4403 by default
(`[computer] exit_port`, `PAGIS_COMPUTER_EXIT_PORT`). It reads one head of
at most 8 KiB within 10 seconds, and the token check is the first thing
that it does with it: a token of no awake Agent's Computer gets 407 and
the connection closes. The token names the Computer, and with it the
Agent and the Workspace of its Person. Then:

- A destination that is an address and not a public unicast address
  gets 403: loopback, private, link-local, carrier-grade NAT, multicast,
  unspecified, broadcast, reserved and the documentation ranges.
- When the Person's Home Exit is present, the listener opens one stream
  to it, and answers 200, 403 for a destination that the Home Exit
  refused, or 502.
- When it is absent, the listener resolves the name on the server,
  refuses every address that is not public unicast with 403, dials from
  the server to the first address that takes the connection, and answers
  200 or 502.

After a 200 the listener copies the bytes both ways until either side
closes. It counts the bytes that each Person's Home Exit carries, in
memory.

The listener binds every interface of the server. The Computers reach it
at `host.docker.internal`, which Docker maps to an address of its own
choice on the Docker host, so no one bind address fits every Docker host.
The egress rules of the deployment close the port to everything but the
Computers: a rule of the `INPUT` chain drops the TCP of the port on every
interface but the Computers' bridges, so the network, the other
containers and the server itself reach it no more than they reach a
closed port. The token check stands in for a narrower bind among the
Computers: the token is 256 random bits that the agent's shell cannot
read. One source address holds at most 256 connections at once, eight
times the 32 that Chromium holds to one proxy, so a shell of an Agent,
which reaches the port with no token, cannot take the open files that
serve every Workspace.

### The Home Exit is one Host of the Person, chosen by that Person

The **Home Exit** is the Host through which a Person's Computers reach the
internet. A Person turns it on in Settings and chooses one of their own
Hosts that declares the `exit` capability. Only a Host of the same Person
can be the Home Exit of that Person's Computers, so an Agent never leaves
through another Person's machine. An Administrator can turn the Home Exit
off for the whole installation with a System Setting, and can never turn it
on for a Person.

The Workspace holds the choice: one Host of that Workspace, or none. The
store writes a Host of the same Workspace alone, in the one statement of
the write. The daemon checks it again when it opens a stream: an exit
socket carries the connections of the Workspace whose Session opened it,
and of no other, whatever the record names.

The Person reads and sets the choice at `/api/v1/settings/home-exit` of
the product port: `GET` reads it with the Hosts that declared `exit` and
whether each has its exit socket open, `PUT` chooses a Host, and `DELETE`
clears the choice. `PUT` reads the Host in the Person's Workspace, so the
Host of another Person answers `404`, and a Host that declared no `exit`
answers `422`. The Settings card is on the Hosts page of the Product App.
A Local Installation has no Home Exit: `GET` says that it is not
available, the card shows nothing, and a change answers `409`.

The **Home Exit System Setting** is `[computer] home_exit` of
`config.toml`, on by default. An Administrator turns it off in the System
Settings of the Administration Interface (`PUT
/api/v1/settings/system/home-exit`), which a Local Installation answers
with `409`. While it is off, no choice is in effect: every Computer runs
in Direct mode, the exit listener carries no connection through a Home
Exit, and a new choice answers `409`. The choice of each Person stays in
the store, the Settings card says that the Administrator turned the Home
Exit off, and each choice is in effect again when the setting is on.

### A change switches the awake Computers at once

When a Person turns their Home Exit on, off, or to another Host, the
daemon switches the Exit Proxy of each of their awake Agent's Computers
with `POST /exit`, at once and with no restart. It switches every one,
also one whose mode stays, so the connections through the old Host close.
A Computer whose switch fails keeps its mode and does not stop the others:
the answer of the route names each one with the reason, and that Computer
takes the choice at its next wake. A Computer that is asleep takes it at
its wake.

When the System Setting changes, the daemon switches each awake Computer
of every Person whose mode is not the mode in effect, and leaves the others
and their connections as they are. Its answer counts the Computers that
did not switch, and the log names them.

A Computer that downloads its image or starts when the choice changes read
the choice at its wake, and a switch passes it by. Once it is awake, the
daemon compares its mode with the mode in effect and switches it when they
differ. A Computer that a restart of the daemon left running reports its
mode at its adoption, and the daemon switches it the same way. The switches
of one Workspace run one at a time, so the last change of the choice is
the one that the Computers keep.

A Client App that is connected to an installation that it did not start
declares `exit` beside `shell`. A Client App of a Local Installation does
not. After its Host socket registered the machine, the Client App opens
the exit socket, a second WebSocket at `/api/v1/hosts/{host_id}/exit`,
with the same Session cookie and the same trusted-origin rule as the Host
socket, so bulk bytes never wait in front of a `host_shell` dispatch. The
daemon refuses the socket of a Host of another Workspace, and of a Host
that declared no `exit`. The Host is present as a Home Exit while this
socket lives. The socket reconnects as the Host socket does, and it closes
with 1008 at the end of its Session.

Binary frames carry one byte stream, and yamux runs over it. The daemon
opens one stream for each connection and the Client App accepts it. The
daemon runs the `yamux` crate of libp2p. The Client App runs a module of
its own with the accepting side of the yamux specification: the Node
yamux of libp2p needs a libp2p connection and about thirty packages, and
the other Node yamux loses bytes across chunk boundaries and has no
receive backpressure. A test runs the two against each other over the
real WebSocket. Each stream starts with one line each way:

1. The daemon writes the preamble: the destination as `host:port` and a
   line feed, at most 262 bytes. The host is a DNS name, an IPv4
   address, or an IPv6 address in brackets, as in the target of a
   `CONNECT`.
2. The Client App answers with one status line before any other byte:
   `ok`, `refused <reason>` or `failed <reason>`, at most 512 bytes.
   `refused` means that every address of the destination failed the
   address check, and `failed` means a malformed preamble, a name that
   does not resolve, or a connection that failed.
3. After `ok` the stream carries the raw bytes of the connection both
   ways, and a half-close of one side is a half-close of the other.

For each stream, the Client App resolves the name and refuses, after
the lookup, each loopback, private, link-local, carrier-grade NAT,
multicast, unspecified or broadcast address, IPv4-mapped and NAT64 forms
included, each address of its own machine, and each address in the
subnet of one of its interfaces, in both families. A home network can
have public addresses: most have global IPv6 addresses, and some
machines have a public IPv4 address, so the fixed ranges alone do not
hold it. The Client App reads its interfaces at each dial, because they
change as a laptop moves. It dials the first address that passes. The
check after the lookup stops a name that resolves to the home network.

When the Home Exit is absent, a new connection leaves from the server, and
the connections that it carried close: the end of the exit socket ends
every stream on it.

### The Person sees the exit in use

The daemon defines the **exit in use** of an awake Agent's Computer once,
for every place that shows it:

- In Home mode while the Person's Home Exit is present: `exit: <Host
  name>`, such as `exit: MacBook Pro`.
- In Home mode while it is absent: `exit: server`.
- In Direct mode: nothing, because a Person with no Home Exit needs no
  label.

The view of the Computer reads it as the `exit` of its state, and the
Agent reads it on the last line of each `computer` tool result, outside
the envelope of the screen, because the daemon writes it. A sudden change
of address is a signal that sites read, so the Person sees each change: the
daemon publishes `computer.exit_changed` with the new label for each
Computer that switches, and for each Computer in Home mode whose Home Exit
comes or goes, and the Product App reads the state of that Computer again.

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
byte count, and its menu turns the Home Exit off. The Client App counts in
its own carry of each stream: the connections open now, and the bytes that
it copied both ways since it started. While a connection is open, the first
item of the tray menu says "Home Exit: 3 connections, 12.4 MB carried", and
"Turn Off Home Exit" under it sends `DELETE /api/v1/settings/home-exit`
with the Session of the Host socket, as the Settings card does.

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
- Chromium's local network checks see the proxy and not the destination,
  because Chromium resolves no name that it sends to a proxy. A page's
  request to a name that resolves to a private address therefore reaches
  the other containers of the Workspace and the private destinations
  that the Computer reaches: the blocks of `PAGIS_COMPUTER_ALLOW` on a
  server, and the LAN on a Local Installation, which has no egress
  rules. It reaches nothing on the Computer itself.
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
- Through a Home Exit a name resolves at the Person's home, so an
  internal name of the server's network, such as one that
  `PAGIS_COMPUTER_ALLOW` opens, does not resolve there. A Computer in Home
  mode reaches such a destination by its address, which leaves from the
  Computer.
- A change of the choice closes the open connections of each awake
  Computer of the Person, also when the mode stays, and a change of the
  System Setting closes those of each Computer that switches. A Computer
  whose switch fails keeps its mode until its next wake.
- In Home mode every connection of a Computer but a literal private one
  passes through the daemon, so a new connection fails while the daemon
  restarts, and the connections that it carried close with it.
- The exit listener is a port of the server on every interface. On a
  server that the deployment's egress rules do not hold, the firewall of
  the server keeps it closed to the network.
- The Client App's check holds the subnets of its interfaces, and not a
  home network that it reaches through a router only, such as a second
  subnet behind the same router.
