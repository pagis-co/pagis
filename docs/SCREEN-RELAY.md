# The screen Media Relay

A browser watches an Agent's Computer through the Media Relay. A Computer
publishes no media port at all, so a browser never sends media to a container.
It sends media to the relay, and the relay copies the packets to the Computer
and back. The Computer registers outbound with the relay for each viewer
session, from inside its Tenant Network, so the relay learns the Computer's
address from the Computer itself.

The `[screen]` section of `config.toml` names the relay. ADR-0014 holds the
decision behind it.

## A local installation

Set nothing. The relay advertises `127.0.0.1` and the browser is on the same
machine. A local installation in the multi-user mode names the address that
the other People reach it at; "The live screen for other people" in
`docs/DEPLOYING-A-SERVER.md` gives it for each proxy and tunnel.

## A server with a public address

One advertised address and one UDP port range serve every Computer of the
installation, so a firewall rule and a compose port list name them once.

```toml
[screen]
# The address browsers resolve for this server. The relay advertises it in
# every screen session's ICE candidate.
advertise_ip = "203.0.113.10"
# The UDP ports the relay may bind. One port carries one viewer, so this range
# is how many people may watch a screen at once.
media_port_first = 50000
media_port_last = 50099
# The daemon forwards the media itself. This is the default.
relay = "daemon"
```

Open `media_port_first` to `media_port_last` for UDP, and nothing else. The
daemon needs no other inbound port for a screen. The Computers register with
the relay on the same range from the Docker bridges, so a firewall that
filters traffic from the bridges must accept that range from them too.

`PAGIS_SCREEN_ADVERTISE_IP`, `PAGIS_SCREEN_MEDIA_PORT_FIRST` and
`PAGIS_SCREEN_MEDIA_PORT_LAST` set the same three for one run. A container
deployment states them there once and publishes exactly that range; see
`docs/DEPLOYING-A-SERVER.md`.

## A server behind an external TURN server

Use this where the firewall or the load makes a daemon-owned public UDP range
wrong. The browser reaches the relay through the TURN server, so the public UDP
address and ports belong to that server.

```toml
[screen]
# The address the TURN server reaches this daemon at.
advertise_ip = "10.0.1.7"
media_port_first = 50000
media_port_last = 50099
relay = "turn"

[screen.turn]
urls = ["turn:relay.example.net:3478?transport=udp"]
# The same value as coturn's static-auth-secret.
secret = "the-shared-secret"
# How long one minted credential lives.
ttl_seconds = 3600
```

The daemon mints one credential for each viewer session under the TURN REST API
long-term credential scheme, which is coturn's `use-auth-secret` mode: the
username carries the expiry and the password is its HMAC under the shared
secret. No account exists on the TURN server, and a credential that leaks
expires. The matching coturn settings are:

```
use-auth-secret
static-auth-secret=the-shared-secret
realm=relay.example.net
```

## What the daemon refuses at boot

- A `relay` that is not `daemon` or `turn`.
- `relay = "turn"` with no `urls` or no `secret`, because the daemon would have
  nothing to mint a credential for.
- A `media_port_first` above `media_port_last`, or a first port of zero.

## When a viewer sees no screen

- Every port of the range is in use. The daemon refuses the next viewer and
  says so; widen the range.
- A browser reaches the advertised address on TCP but not on UDP. The range is
  UDP only, and the payload is DTLS-SRTP.
- The advertised address is the one the machine holds, not the one browsers
  resolve. The relay advertises exactly what this setting says.
- The Computer's registration does not reach the relay. A Computer reaches the
  daemon as `host.docker.internal`; a firewall that refuses UDP from the Docker
  bridges to the range stops the registration, and the relay then has no
  Computer to copy the browser's packets to.
