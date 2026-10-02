# 0028: Remote Access runs through the owner's Tailscale, and a client signs in with a Sign-In Link

Status: accepted.

## Context

A person who runs Pagis at home wants to reach it from their phone and from
a laptop away from home. Other People of the household want to reach it too.
The installation sits behind a home router with no public address, often
behind carrier-grade NAT. The Multi-User Mode that ADR-0024 first recorded
asked the owner to set up a proxy or a tunnel by hand, type its name as the
Public Origin, and let every other machine sign in with a password on a
page that faces the internet.

Products that serve a home installation to its owner's phones and laptops
share one shape. OpenClaw turns on Tailscale Serve or Funnel from one
gateway setting. Home Assistant's Remote UI is one switch over the Nabu
Casa relay. Plex and Synology QuickConnect fall back to a vendor relay. Each
connects outbound only, keeps one stable name, and ends TLS on the home
machine. A new phone joins from a screen that is already signed in: OpenClaw,
Paseo and Jellyfin Quick Connect show a QR code or a short code. In OpenClaw
the code is good for ten minutes and one use, and the phone then holds a
credential of its own that the owner can revoke. Paseo's pairing link is a
lasting owner secret with no list of the phones that used it, so a photo of
the QR code is lasting access.

## Decision

### Remote Access is a public name from the owner's Tailscale Funnel

**Remote Access** is how an installation at home serves the owner's other
machines and the other People of the installation: a public `https://` name
that the owner's Tailscale Funnel answers on, and a product port that signs
a client in only with a Sign-In Link.

On a Local Installation the daemon drives the Tailscale of its machine,
through the `tailscale` command, as it runs `git`. It runs as the owner, on
the owner's machine, so it needs no help from the Client App. One switch in
the Administration Interface, under Network, turns Remote Access on:

1. The daemon reads the Tailscale state. With no Tailscale, or Tailscale
   signed out, the switch says what to install or open.
2. Where the tailnet has HTTPS or Funnel off, the switch shows the page
   that Tailscale names to turn it on, and waits.
3. The daemon turns on Funnel on port 443 to the product port on loopback,
   and on port 8443 to the TURN server of the live screen on loopback.
   Where either port serves something else, the switch says so and
   replaces nothing. Where Tailscale refuses Funnel on macOS, the switch
   names the standalone build.
4. It writes the Public Origin, `https://<machine>.<tailnet>.ts.net`, and
   `127.0.0.1` as the Trusted Proxy, and asks for the reserved restart.

Turning the switch off removes both Funnel ports of Pagis, clears the
three settings, and asks for the restart. `config.toml` records Remote
Access as `[remote_access] enabled`, and `PAGIS_REMOTE_ACCESS` sets it for
one run.

The Funnel relays route on the TLS server name and decrypt nothing. The
`tailscaled` on the owner's machine holds the certificate and forwards to
the daemon on loopback, so the daemon still terminates no TLS (ADR-0024).
It sets `X-Forwarded-For` to the browser's address, `X-Forwarded-Proto` to
`https`, and `Tailscale-Funnel-Request` on a request from the internet. It
carries a WebSocket upgrade and streams server-sent events as the daemon
writes them.
The Bind Address stays loopback. A phone needs no Tailscale app, and so
keeps its one VPN slot for another use.

A Headless Server at home gets the same name from an optional `tailscale`
service in `deploy/compose.yaml`, which runs the official Tailscale image
with a Funnel configuration. The deployment sets `PAGIS_REMOTE_ACCESS`.

Other ways were considered:

- **A Pagis relay**, as Nabu Casa runs. It is the shortest setup, but Pagis
  would run wildcard DNS, a certificate path for each installation, a relay
  fleet, TURN and abuse handling, and each home machine would need a TLS
  terminator beside the daemon.
- **Cloudflare Tunnel.** It needs a Cloudflare account and a domain whose
  DNS is on Cloudflare, Cloudflare ends TLS, and it carries no UDP.
- **Cloudflare Quick Tunnels and the ngrok free plan.** Cloudflare offers
  Quick Tunnels for tests alone, with a new name at each start and no
  server-sent events. The ngrok free plan carries 1 GB a month.

### Remote Access takes the place of the Multi-User Mode

A Local Installation serves other machines only through Remote Access. The
hand-made path goes: the Multi-User Mode switch, the Public Origin field,
and the Caddy, Tailscale Serve and Cloudflare Tunnel setups of the
documentation. "Just me" and "Several people" in the Client App setup
(ADR-0025) say only whether the owner will create other People.

A Server behind its own proxy (ADR-0024) is not in Remote Access. It keeps
the Public Origin that its deployment names and the password sign-in.

### A client signs in with a Sign-In Link, never with a password

With Remote Access on, the product port accepts no password from another
machine, and the refusal names the Sign-In Link. The sign-in page reads
from the health answer how its browser signs in: a browser on another
machine gets one field for a pasted link, with the line that says where to
get one, and a browser on this machine keeps the password. A browser or an
app signs in with a **Sign-In Link**: a one-use
URL of the Public Origin, `https://<public origin>/sign-in#<secret>`,
shown as a QR code beside a copy button. The secret is in the fragment, so
it reaches no proxy log and no `Referer`. The page posts it to the daemon,
so a message app that opens a link to make a preview does not spend it.

Three things make a Sign-In Link:

- **A Person signs in one more client.** In Settings, under Sessions, a
  signed-in Person makes a link for one more browser or app of their own.
  It is good for five minutes and one use.
- **An Administrator invites a Person.** Creating a Person makes a link for
  that Person's first Session. It is good for seven days and one use.
- **`pagis pair`** on the machine of the installation prints a link and its
  QR code in the terminal. It gives a Headless Server its first Session,
  and it is the way back in for a Person who has no Session left.

The link that the `pagis` binary prints at start for its own machine stays
as it is (ADR-0025).

A spent link trades for a Session named for the client, such as "Pagis on
iPhone" or "Safari on macOS". The Person's Sessions list shows each one,
and removing one ends it. A Session ends 30 days after its last use, not
30 days after the sign-in, because a client with no password cannot sign
in again alone. The trade counts refusals for each address, as the
password sign-in does.

A Client App on another machine connects to an installation in Remote
Access with a Sign-In Link. "Connect to a Pagis server" takes the link,
keeps its origin, and trades the secret for its Session. A Client App that
is connected already, and opens the sign-in page because its link was spent
or its Session ended, gets the field for a pasted link there. The trust
statement of ADR-0015 stays: a client that connects runs what the
installation dispatches.

### The live screen reaches another machine through TURN over the Funnel

The Funnel carries TCP alone, and the Media Relay sends the screen over UDP
(ADR-0014). With Remote Access on, the daemon also runs a TURN server on a
loopback TCP port, `[screen] remote_access_turn_port` (4402 by default).
The turn-on publishes it with
`tailscale funnel --bg --yes --tls-terminated-tcp=8443 tcp://127.0.0.1:<port>`,
so `tailscaled` ends TLS on port 8443 and forwards plain TCP. The port is
fixed and never random, because the Funnel configuration names it. The
TURN server listens on loopback alone, and only while Remote Access is on.

`GET /api/v1/screen/ice` gives a browser that is not on this machine, by
the test of the sign-in rules, `turns:<machine>.<tailnet>.ts.net:8443?transport=tcp`
with a credential of its own. A browser on this machine gets no TURN
server and keeps the direct path. A browser on the tailnet gets the TURN
server too, and where the Media Relay advertises an address that it
reaches over UDP, ICE picks that direct pair first.

The browser makes a UDP allocation over its TCP connection (RFC 8656), and
the server relays its datagrams to the Media Relay. The server relays to
the Media Relay's advertised address, on a port of its UDP range, and to
nothing else: it refuses another port of loopback, another address of the
machine and every address of the internet, so the public port reaches
nothing else on this machine. Each relay socket binds the Media Relay's
address, at a port outside its range. A credential follows the TURN REST
API scheme of the `turn` relay, under a secret that the daemon makes at
each start and keeps in memory, out of `config.toml`. A credential lives
twelve hours, because a browser signs each Refresh of its allocation with
it.

The TURN code is the `turn` crate of webrtc-rs (MIT or Apache-2.0), on its
maintained 0.17 line: the long-term credential check, allocations,
permissions and channels, in the tokio runtime of the daemon. The crate
reads whole messages and has no TCP listener, so Pagis frames each TCP
connection as RFC 8656 section 12.5 says: a STUN message by its length,
and a ChannelData message by its length and its padding to four bytes,
which browsers send over TCP. A connection that closes takes its
allocation with it. The server holds at most 64 connections, closes a
connection that makes no allocation in ten seconds, and keeps at most 1024
nonces, because the crate keeps each nonce that it gives out.

Other ways were considered:

- **turn-rs (`turn-server`).** It is MIT and reads TCP, but it relays only
  between its own clients: a peer must be an address of the TURN server,
  so it cannot reach the Media Relay.
- **turn-server-proto (turn-proto).** It is a sans-IO server for TCP and
  UDP, but its TCP buffer neither strips nor adds the padding of a
  ChannelData message, and Pagis would write the whole I/O loop around it.
- **medea-turn.** It is a fork of the webrtc-rs crate with a TCP
  transport, and its repository is archived.
- **coturn beside the daemon.** It relays UDP behind
  `--tls-terminated-tcp=8443`, but it is a second process for each Local
  Installation, which the Client App would install and supervise.

## Consequences

- The owner needs a Tailscale account and Tailscale on the machine that runs
  the installation. The other machines need neither.
- The name of the machine and the tailnet is in public Certificate
  Transparency logs, and bots find the name. Every route that is not the
  sign-in page, the Sign-In Link trade, the health route or the static
  Product App needs a Session. The Administration Port stays on loopback
  and is never on the Funnel.
- A Person with no Session left signs in again only through a new link: a
  link from another of their Sessions, an invite from an Administrator, or
  `pagis pair` on the machine. The owner of a Local Installation always has
  the Client App, which trades the Client Credential.
- Tailscale does not publish the Funnel bandwidth limit. Through the public
  relays, one machine with a 150 Mbit/s upload carried 5 to 12 Mbit/s on
  one connection and about 15 Mbit/s in total on four. The live screen
  needs about 2 Mbit/s for each viewer, so a few people watch at once.
- The TURN port faces the internet through the Funnel. A client with no
  credential gets no allocation, and an allocation reaches the Media
  Relay alone, where a viewer path takes nothing from a sender that does
  not hold its ICE password.
- A round trip through a Funnel relay and back to the same machine took
  120 to 265 ms, for TURN and for a WebSocket. A phone pays the path from
  the phone to the relay and from the relay to the home machine.
- The standalone build of Tailscale for macOS serves Funnel. Tailscale's
  documents disagree on whether the Mac App Store build does.

## Not built

- The `tailscale` service of `deploy/compose.yaml`. A Headless Server in
  Remote Access runs the TURN server, and nothing publishes its port.
