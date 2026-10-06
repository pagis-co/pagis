# 0030: A Notification is a Web Push to each Push Subscription of the Person

Status: accepted.

## Context

A Request parks a Run until the Person answers it (ADR-0004). The Person
learns about it only while a browser tab or the Client App holds the event
socket, `/api/v1/ws`, open. With no client open, nothing reaches the Person,
and the Run waits until the Person comes back to the desk. People answer
that kind of question on a phone.

The Client App cannot fill the gap. Its product window refuses the
notifications permission (`desktop/src/webPermissions.ts`), and Electron
gives a page no push service. The Needs-You Queue exists only in the UI:
`buildQueue` in `ui/src/components/home/queue.ts` derives it from the
pending Requests, the live Runs that wait for the Person, the Keypad Code
card, the missed Calls and the failed Runs of today. So the daemon cannot
say what needs the Person, and it cannot decide a Notification or a badge
count.

Products that reach a person away from the desk share one shape:

- Mastodon sends standard Web Push. Its iOS and Android apps receive it
  through a relay, `webpush-apn-relay` and `webpush-fcm-relay`.
- UnifiedPush uses Web Push, RFC 8291 and VAPID, as its protocol.
- Nextcloud, Bitwarden, Matrix Sygnal and Home Assistant each run one
  relay for their store apps, because the APNs and FCM keys belong to the
  publisher of the app.
- The ntfy relay forwards a message and never reads its content.
- Slack holds a push while the person is active on the desktop. Claude
  Code Remote Control holds a push while the person is active at the
  terminal.
- WebKit's Declarative Web Push, on iOS 18.4 and later, shows a JSON push
  on a Home Screen web app with no service worker code. A service worker
  that fails therefore never costs the subscription.

## Decision

### The daemon derives the Needs-You Queue

One derivation in the daemon holds the Needs-You Queue. It holds the five
kinds that the UI holds, in the same order: `approval`, `waiting`,
`keypad`, `call` and `failed`. Home, the sidebar count, the app badge and
each Notification read this one derivation. The UI derives nothing, and
the queue stays a view, never a second source of truth (ADR-0022).

### The ring rule decides what sends a Notification

A ring calls, and a caption reports (`docs/UI-DESIGN.md`). A
**Notification** goes only when an item enters the Needs-You Queue. A
Report, a caption, a Run that completes and a Reflection send none.

### One transport: standard Web Push

The daemon sends each Notification as a standard Web Push. Delivery is
RFC 8030. Encryption is RFC 8291 `aes128gcm`. The sender identity is
RFC 8292 VAPID. A push service of a browser and the Push Relay are equal
endpoints, so a store app gets Web Push through the Push Relay, as a
browser gets it from its push service. The daemon holds no APNs or FCM
code and no APNs or FCM key.

The `web-push-native` 0.5 crate builds the encrypted body and the VAPID
header with pure RustCrypto and no OpenSSL. The workspace `reqwest`, with
`rustls-tls`, sends it. The `web-push` crate is not used, because it
needs OpenSSL through `ece`.

No text message from Pagis to the Person is a notification channel.

### A Push Subscription belongs to one Session

A **Push Subscription** holds the push endpoint of one client, its P-256
public key and its auth secret. It belongs to one Session, and it ends
when that Session ends. A push service that answers `404` or `410` to a
send ends it too.

### One VAPID Key for each installation

The **VAPID Key** is one P-256 key pair for each installation. It signs
each Web Push of the installation. The private half is one named entry of
`secrets.enc`, through `SecretStore` (ADR-0013), and not a Credential of
the Vault. The daemon makes it at the first need.

### The payload is a Declarative Web Push message

The plaintext of each Web Push is the WebKit Declarative Web Push JSON:

- `"web_push": 8030`.
- A `notification` member with `title`, `body`, `navigate`, `app_badge`
  and `data`. `navigate` is the place of the item on the Public Origin.
  `app_badge` is the count of the Needs-You Queue.
- `"mutable": true`.

The Pagis fields are in `notification.data`:
`{v, item, kind, request?: {id, actions}}`.

- `v` is the version of the payload format, `1`.
- `item` is the id of the queue item.
- `kind` is a queue kind, or `test`.
- `request` is there only for a pending Request that a Notification can
  answer: a tool action or a credential action Approval. Its `actions`
  are `approve_once` and `deny`.

The service worker of the Product App, the iOS Notification Service
Extension and the Android messaging service of the Mobile App (ADR-0032) parse
the same JSON. A client shows **Approve once** and **Deny** where it can: the
Mobile App on iOS and Android, and the service worker where the browser
shows notification actions, as Chrome and Edge do. Safari shows no
actions, so there a tap opens the item. An answer from a Notification
goes to the decision route of the Request,
`POST /api/v1/requests/{request_id}/decision`, with the scope `once`. A
Notification never carries an answer that writes an Allow Rule, so
`actions` holds no `always`.

The plaintext is at most 2048 bytes. The encrypted body is at most 2800
bytes, so it fits the 4096 bytes of APNs and FCM after base64 and the
envelope of the Push Relay. Only the client decrypts. A push service and
the Push Relay see the ciphertext, its size and its time.

The payload format and the decision route are a stable contract between a
server and the store apps of the Mobile App. A store app and a
self-hosted server update at different times, so this contract is the one
exception to the rule of no backward compatibility. A change that breaks
it raises `v`, and a Mobile App release that reads the new `v` ships
before a server sends it. A client that does not know `v` shows a
placeholder Notification, and a tap on it opens the app.

### Headers

Each Web Push carries:

- `TTL: 86400`.
- `Urgency: high` for `approval`, `waiting` and `keypad`, and
  `Urgency: normal` for the other kinds.
- `Topic` from the item id, in at most 32 URL-safe base64 characters
  (RFC 8030 section 5.4). A newer push of the same item replaces a push
  that the push service did not deliver.

### No dismissal push

iOS shows every Web Push, so a push that only removes a Notification
shows a Notification. The daemon sends no push when an item leaves the
Needs-You Queue. When a client opens, it clears its delivered
Notifications whose items left the queue, and it sets its badge from the
daemon's queue.

### Hold while active

A new item waits while the Person used any client in the last 120 s. A
client that is visible sends an `activity` frame on the event socket after
input, at most once every 30 s. The daemon keeps the time of the last
`activity` frame for each Workspace in memory, as it keeps the Presence of
a Host (ADR-0015). A `ping` or a query does not count, because a hidden
tab sends them too. The `last_used_at` of a Session cannot tell this,
because the daemon moves it at most once an hour.

When 120 s pass with no activity, the daemon sends each held item that is
still in the queue. A restart of the daemon forgets the time, so an item
after a restart goes at once.

### Endpoint guard

The daemon posts only to an `https` endpoint with a DNS name. Each address
of the name must be public at the time of the send, by the test of
`is_public_unicast` (ADR-0029). The daemon connects to an address that it
checked, and does not resolve the name a second time.

### The Push Relay

The **Push Relay** is one relay that the project runs for its store apps.
It is open source, in this repository. It holds the APNs and FCM keys. It
gives each installation of the Mobile App an opaque `https` endpoint, and
binds the endpoint to the VAPID Key that the Mobile App names when it
registers. For each Web Push it checks the VAPID token, the size and the
rate, and forwards the ciphertext to APNs or FCM. It never holds a key
that decrypts a payload.

The relay is the crate `pagis-push-relay`, a library and a binary. It
depends on no daemon crate. It reads its own `PUSH_RELAY_*` settings and
keeps its registrations in its own SQLite file, because it runs apart
from every Pagis installation.

An installation registers with `POST /v1/registrations` and sends its
platform (`ios` with an APNs environment, or `android`), its device token
and the VAPID Key of its server. The relay answers a random id, a secret
and the endpoint `<origin>/v1/push/<id>`:

- The endpoint holds the random id and never the device token, so a
  leaked endpoint names no phone. Mastodon's `webpush-apn-relay` puts
  the device token in the URL, and the relay does not copy that.
- One registration binds one VAPID Key, as Mozilla autopush binds a
  subscription to the key that made it.
- The relay keeps only the SHA-256 of the secret. The secret changes the
  token when APNs or FCM rotates it, and removes the registration. A
  wrong secret and an unknown id get the same `404`, so a caller cannot
  find which ids exist.
- One client address makes at most 20 registrations in each hour. The
  relay counts in process, in a fixed window. It reads the last entry of
  `X-Forwarded-For` only from the proxy address that
  `PUSH_RELAY_TRUSTED_PROXY` names, by the rule of the daemon's Trusted
  Proxy.
- A log line holds the route, the id and the status, and never a token,
  a secret or a VAPID Key.

## Consequences

- A Person reads a Request on a phone with no tab open.
- The daemon sends HTTPS to the push services of the browsers and to the
  Push Relay. A server with a closed egress must allow them.
- A Local Installation on a computer that sleeps sends nothing until it
  wakes.
- Safari on iOS gets Web Push only for a Home Screen web app.
- The Client App gets no Notification, because Electron gives no push
  service. The Person at the Client App has the queue, the sidebar count
  and the sound cues.
- The Push Relay is a service that the project runs. When it is down, the
  Mobile App gets no Notification, and a browser still gets one.
- A push service and the Push Relay learn when the installation sends a
  Notification and how large it is, but not what it says.
- A Person who answers from a Notification approves once. A standing
  Allow Rule still needs the card in a client.
- Each held item goes 120 s after the last activity, so a Person who
  walks away from the desk gets a Notification after that delay.

## Not built

- The derivation of the Needs-You Queue in the daemon. The UI derives it.
- The Push Subscriptions.
- The VAPID Key.
- The sender: the payload, the headers, the endpoint guard and the end of
  a Push Subscription on `404` or `410`.
- The hold while active, and the `activity` frame.
- The service worker of the Product App.
- The Push Relay takes no Web Push: the push route, the check of the
  VAPID token, the size and the rate, and the forward to APNs and FCM.
  The relay registers an installation and gives it an endpoint.
- The Mobile App (ADR-0032).
