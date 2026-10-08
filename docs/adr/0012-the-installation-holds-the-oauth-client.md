# 0012: The installation holds the OAuth client, and the daemon serves the catalog

Status: accepted.

## Context

A Connection needs a credential. There are three ways to get an OAuth one.

- **Each person registers a client.** The person registers a client with the
  provider and gives Pagis its id and secret. The daemon runs the loopback flow
  of RFC 8252 section 7.3 on its own host. Every person does console work for
  each provider, and only a person at the daemon host can finish the flow.
- **The installation registers one client.** An Administrator registers one Web
  OAuth client for the installation. Each person signs in at the provider and
  allows Pagis. The console work happens once for the installation, and the
  flow works from any browser that reaches the Public Origin.
- **A token relay.** Pagis registers one client and runs a small service that
  makes the token call for every installation. The person clicks one button,
  and the provider's terms and limits apply to that one client as a whole.

Products that connect a person's work account use the second way. The person
sees only the provider's account chooser and consent screen, and an
administrator owns the client: Home Assistant's application credentials,
Nextcloud's Google integration and a Slack or Google Workspace app that an
administrator installs for a workspace.

A desktop-app client secret is not a secret: Google says so, RFC 8252 section
8.5 agrees, and Thunderbird and the GitHub CLI ship theirs. Loopback and PKCE
solve the protocol. The real limits are policy and cost: a shared client makes
Pagis one OAuth app for all users, which spends a lifetime user cap, makes
verification mandatory, and adds a yearly assessment for a restricted scope.
Raycast and Nabu Casa both relay only where that cost is low.

## Decision

### The installation's own client is the floor under every OAuth provider

Each OAuth provider gets a working path through an Installation OAuth Client
first. An Administrator registers it once in the Administration Interface
(ADR-0024). On a Local Installation the owner is the Administrator. A person
never types a client id or a client secret: the person signs in at the provider
and allows Pagis.

A relay is an optional layer on top and never the only way in, so a suspended or
saturated shared client leaves every installation a way in.

### Four gates admit a provider to a relay

1. **A shared client is permitted.** The developer terms let one client serve
   many independent installations and allow credentials in open source.
2. **Custody is bounded.** The relay holds at most a client secret for the
   token call, holds no user token at rest, and is never the only control that
   stops an intercepted code from being redeemed.
3. **Loss is survivable.** An Installation OAuth Client works for the provider.
4. **The relay is much better.** An Installation OAuth Client costs one
   Administrator console work, not a two-field token paste; where it is a
   personal access token, a relay adds custody and buys nothing.

These are properties of the provider, and money does not change them. A
provider without PKCE is not excluded; gate 2 applies. Pagis keeps no
candidate list and tests a provider when someone proposes it.

A user cap, a verification and a yearly assessment are prices. They decide
when a provider gets a relay, not whether. Before any relay ships, Pagis needs
a legal entity, a published privacy policy, a verified consent-screen domain,
a budget and a named owner for any restricted-scope assessment, and a named
owner for the yearly revalidation.

### A broker gives each OAuth Connection its credential

A Connection of an OAuth provider holds a credential that a broker gave it. The
broker registered the OAuth client, and the person gave their own consent
against it. Every person consents against the same client, each with their own
consent and their own credential; a revocation at the provider touches one
person alone. A Connection of a key provider holds the key that a Person or an
Administrator supplied. The kind of the Provider Catalog entry, `oauth` or
`fields`, says which, so a Connection records no auth mode.

There are two brokers:

- **The installation.** An Administrator sets up one Web OAuth client, the
  Installation OAuth Client: the id on the Org record, the secret as an
  installation secret in `secrets.enc`. It is the `oauth-client` part of the
  Google entry of the Provider Catalog, set up in the Administration Interface
  (ADR-0024). The daemon runs the authorization-code flow on its own hostname
  and seals each person's refresh token with that person's Tenant Data Key
  (ADR-0013).
- **Pagis**, through a Token Relay. The four gates and the relay requirements
  apply to this case alone.

The broker can withdraw the client, and every Connection of that provider then
needs a new consent.

Where the Org holds no Installation OAuth Client, the Google entry of the
person's catalog says that an Administrator sets up Google sign-in in the
Administration Interface and offers no way on, and the daemon refuses a new
Google Connection with the same step.

Where the Org holds one, a new Google Connection carries its alias and its
display name, and no account. The authorize request returns the address to
send the person to at once. A public callback route on the Public Origin
receives the redirect. A `state` value, made for each authorization, names one
Connection and one Person. A start route on the Public Origin requires a
Session of that Person and sets a transaction cookie that binds the `state` to
that browser; on a Local Installation with Remote Access off, it requires no
Session, because that installation has one Person. The callback refuses a
redirect without that cookie or with a Session that is not live.

Google's account chooser picks the account. The first authorization of a
Connection sends `prompt=select_account consent` and no `login_hint`, and the
daemon records the account that consented, in lower case, as the Connection's
account. The
daemon refuses an account that another Connection of the same Workspace holds.
A later authorization sends the Connection's account as `login_hint`, and the
daemon seals the token only when the consenting account is that account. The
Connection records only the requested capabilities that Google granted.

The daemon gives `gog` a fresh access token for each call through
`GOG_ACCESS_TOKEN`, and `gog` keeps no token. The daemon runs `gog` under the
Workspace's own `GOG_HOME` with `GOG_KEYRING_BACKEND=file` and a password that
the daemon holds in its secret store, so `gog` never reads or writes the
platform keyring, which on macOS is one keychain for every Workspace.

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
provider copy. The daemon serves it, and the client holds no list. An `oauth`
entry has no field: the person types nothing that the provider's own sign-in
asks for.

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
- No person connects Google until an Administrator registers the Installation
  OAuth Client. On a Local Installation the owner does this once.
- The Google Cloud project of the Installation OAuth Client is one OAuth app
  for every person of the installation. Its user cap and its verification
  apply to that project. Google expires a refresh token after seven days while
  the consent screen is in Testing, so the Administration Interface tells the
  Administrator to publish it, or to make it Internal for a Google Workspace
  organization.
- Every OAuth Connection depends on a client the person does not own. When the
  installation's client is removed, every Connection of that provider needs a
  new consent, and the Administration Interface says so where the client is
  removed.
- The daemon holds refresh tokens at rest, sealed with the owning Workspace's
  Tenant Data Key, so one opened row exposes one person.

## Not built

The Token Relay. The broker of every OAuth Connection is the installation.
