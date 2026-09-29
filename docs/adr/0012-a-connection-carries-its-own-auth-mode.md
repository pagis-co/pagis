# 0012: A Connection carries its own auth mode, and the daemon serves the catalog

Status: accepted.

## Context

A Connection needs a credential. There are two ways to get an OAuth one.

- **Bring your own.** The user registers a client with the provider and gives
  Pagis its id and secret. The daemon runs the loopback flow of RFC 8252
  section 7.3: it binds `127.0.0.1` on a random port, opens the system browser
  and exchanges the code with PKCE. The user does console work once for each
  provider.
- **A token relay.** Pagis registers one client and runs a small service that
  makes the token call for every installation. The user clicks one button, and
  the provider's terms and limits apply to that one client as a whole.

A desktop-app client secret is not a secret: Google says so, RFC 8252 section
8.5 agrees, and Thunderbird and the GitHub CLI ship theirs. Loopback and PKCE
solve the protocol. The real limits are policy and cost: a shared client makes
Pagis one OAuth app for all users, which spends a lifetime user cap, makes
verification mandatory, and adds a yearly assessment for a restricted scope.
Raycast and Nabu Casa both relay only where that cost is low, and Home
Assistant reaches Google with bring your own.

## Decision

### Bring your own is the floor under every provider

Each provider gets a working bring-your-own path first. A relay is an optional
layer on top and never the only way in, so a suspended or saturated shared
client leaves every installation a way in.

### Four gates admit a provider to a relay

1. **A shared client is permitted.** The developer terms let one client serve
   many independent installations and allow credentials in open source.
2. **Custody is bounded.** The relay holds at most a client secret for the
   token call, holds no user token at rest, and is never the only control that
   stops an intercepted code from being redeemed.
3. **Loss is survivable.** The bring-your-own floor exists for the provider.
4. **The relay is much better.** Bring your own costs console work, not a
   two-field token paste; where it is a personal access token, a relay adds
   custody and buys nothing.

These are properties of the provider, and money does not change them. A
provider without PKCE is not excluded; gate 2 applies. Pagis keeps no
candidate list and tests a provider when someone proposes it.

A user cap, a verification and a yearly assessment are prices. They decide
when a provider gets a relay, not whether. Before any relay ships, Pagis needs
a legal entity, a published privacy policy, a verified consent-screen domain,
a budget and a named owner for any restricted-scope assessment, and a named
owner for the yearly revalidation.

### A Connection carries its auth mode

The auth mode belongs to the Connection, with the values `byo` and
`brokered`, because one provider can be `byo` on one installation and
`brokered` on another. Admission is tested again at each manifest version. A
provider that stops qualifying gives `byo` to new Connections, and live
Connections continue until their tokens expire.

The two modes differ in who registered the OAuth client. Under `byo` the
person did, and the client reaches the provider once and is never written
down. Under `brokered` the installation did, and every person consents against
the same client, each with their own consent and their own credential; a
revocation at the provider touches one person alone.

There are two brokers:

- **The installation.** An Administrator sets up one Web OAuth client, the
  Installation OAuth Client: the id on the Org record, the secret as an
  installation secret. It is the `oauth-client` part of the Google entry of the
  Provider Catalog, set up in the Administration Interface (ADR-0024). The
  daemon runs the authorization-code flow on its own hostname and seals each
  person's refresh token with that person's Tenant Data Key (ADR-0013).
- **Pagis**, through a Token Relay. The four gates and the relay requirements
  apply to this case alone.

The mode records which credential a Connection carries, not which broker gave
it. Either way the person did not supply the client, so the broker can
withdraw it, and the Connection then needs a new consent.

Where the Org holds an Installation OAuth Client, a new Google Connection is
`brokered`. The authorize request returns the address to send the person to at
once. A public callback route on the Public Origin receives the redirect. A
`state` value, made for each authorization, names one Connection and one
Person. A start route on the Public Origin requires a Session of that Person
and sets a transaction cookie that binds the `state` to that browser; on a
Local Installation with the Multi-User Mode off, it requires no Session,
because that installation has one Person. The callback refuses a redirect
without that cookie or with a Session that is not live, and the daemon seals
the token only when the consenting Google account is the Connection's account.
The Connection records only the requested capabilities that Google granted.

Where the Org holds no client, a new Google Connection is `byo`, and `gog` runs
the loopback flow on the daemon host. Only a person at that machine can finish
it, so the daemon starts it only for a request from that machine, by the rule
of `pagis_server::forwarded::is_from_this_machine` (ADR-0025). Every other
request is refused, including a Member of a multi-user local installation and
every person on a server. The refusal says that an Administrator sets up the
Google OAuth client in the Administration Interface.

`gog` keeps its tokens under the Workspace's own `GOG_HOME`, not in the
platform keyring, so two people who both name a Connection `google` never
share a store. The daemon runs `gog` with `GOG_KEYRING_BACKEND=file` under that
home, with a password that the daemon holds in its secret store and nobody
types. On macOS the platform keyring is one keychain for every Workspace. A
move of the state directory loses those keyrings, and the person connects
Google again.

### Requirements on a relay

1. Refresh carries a ticket made at exchange time. A refresh token alone does
   not prove that this installation made the exchange, and without the ticket
   the relay turns a stolen refresh token into access tokens.
2. Scope comes from a server-side allowlist for each provider, never from the
   caller, so a third party cannot show a consent screen in the Pagis name.
3. Any `code_challenge_method` other than `S256` is refused, because `plain`
   puts the verifier in the browser address.
4. The relay checks that the verifier hashes to the ticket's challenge, so the
   ticket is bound to the authorization it exchanges.
5. The ticket does not ride in `state`, where it leaks a stable installation
   identifier into browser history and provider logs. The daemon holds it in
   memory and posts it directly.

A stateless relay cannot mark a ticket used, so it cannot prevent a replay.
The relay never logs or persists a code, a verifier, a token, a client secret,
or a whole token request or response body.

### The daemon serves the Provider Catalog

`pagis-connect` owns one Provider Catalog: a static list of entries, each with
an id, a label, a blurb, a kind of `oauth` or `fields`, a field list (key,
label, hint, kind, secret, default), a capability set, the capabilities it
declares absent, an optional instance limit, the default name and alias, and
provider copy. The daemon serves it, and the client holds no list.

Each entry declares its installation parts, what the installation sets up once
for everybody: an id, a kind (`connection`, `sip_credential`, `oauth_client` or
`model_key`), copy and fields. An entry with a `connection` part is an
Installation Connection, which the Org owns (ADR-0023).
`installation_setups()` puts the model providers, each with one key part, in
front of the catalog entries, so there is no second registry. The
Administration Interface draws every part from it (ADR-0024). The Product
App's picker lists only the providers a person connects alone.

A create request carries the provider, the alias, the display name and a field
map keyed by the entry's field keys. One parser in the same crate turns the map
into the connector's typed shape, and a test checks that the parser reads every
declared field. A number field travels as decimal text. The connector enforces
the instance limit, and the repair path follows the entry's capabilities, not
the provider name. The UI has one picker grouped by capability, one field form
drawn from the entry, and one OAuth flow. A new provider is one catalog entry
and one parser arm. A provider the catalog does not list gets no capability and
the plain account card.

## Consequences

- No installation depends on a Pagis-run service to reach a provider.
- A `brokered` Connection depends on a client the person does not own. When
  the installation's client is removed, every `brokered` Connection needs a new
  consent, and the Administration Interface says so where the client is
  removed.
- The daemon holds refresh tokens at rest, sealed with the owning Workspace's
  Tenant Data Key, so one opened row exposes one person.

## Not built

The Token Relay. The broker of every `brokered` Connection is the
installation.
