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

The daemon derives the queue on each read from the records, and stores no
copy of it. `GET /api/v1/needs-you` answers `{items, count}` for the
Workspace of the Session. Each item is tagged by its `kind` and has a
stable `id` that names the kind and the record (`request:<id>`,
`run:<id>`, `call:<id>` or `keypad`), its `line`, the `url` of the place
in the Product App that answers it, and the time `at` that orders it
inside its kind. "Today", for a missed Call and a failed Run, is the day
of the Workspace in its time zone.

The daemon publishes an event when an item enters or leaves the queue. A
daemon-lifetime task reads the events that can change the queue: a
Request that opens, is decided or is superseded, a Run that changes state
or is dismissed, a Call that ends or is dismissed, a wrong Keypad Code,
and `keypad.cleared`, which a correct code and the clear in Settings
publish. After each one, the task derives the whole queue of that
Workspace again and compares the item ids with the last set that it holds
in memory. It publishes `needs_you.added` with `{item, count}` for each
new item and `needs_you.removed` with `{item_id, count}` for each item
that left, in the Workspace of the item, so the event socket carries them
to each client of the Person. The task keeps no record. When the daemon
starts, the task derives the queue of each Workspace as its baseline and
publishes nothing, so an item that entered while the daemon was down
publishes no event. A "today" item that leaves at midnight leaves on the
next event of its Workspace, not on a timer.

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

The `web-push-native` 0.5 crate encrypts the body, and the `jwt-simple`
crate that it brings signs the VAPID token, with pure RustCrypto and no
OpenSSL. The workspace `reqwest`, with `rustls-tls`, sends it. The
`web-push` crate is not used, because it needs OpenSSL through `ece`.

### The sender

The crate `pagis-push` holds the Web Push protocol, and only it. It
depends on `pagis-core` and on no other daemon crate. The daemon holds
the Push Subscriptions, the payload and the retry rules.

`WebPush::send` takes a `Subscription` (the endpoint, `p256dh` and
`auth`), the plaintext and the `Options` (`TTL`, `Urgency` and an
optional `Topic`), and answers an `Outcome`:

- `Delivered` for a `2xx` answer;
- `Gone` for `404` or `410`;
- `TooLarge` for `413`;
- `RateLimited` for `429`, with the delay of its `Retry-After` header,
  in seconds or as an HTTP-date;
- `Failed` for every other answer, a refused endpoint and a transport
  error, with the status when an answer came.

A plaintext over 2048 bytes gets `TooLarge` before the encryption, and
nothing goes. The body is one `aes128gcm` record: the plaintext and 103
bytes.

The VAPID token is an ES256 JWT. `aud` is the origin of the endpoint,
with its port when the port is not the default one. `exp` is 12 hours
after the send, apart from the `TTL`. `sub` is the Public Origin when it
is `https`, and `https://github.com/pagis-co/pagis` when it is not,
because Apple refuses a token with no `sub`. The sender signs the claims
with the `jwt-simple` key itself: `VapidSignature::sign` of
`web-push-native` drops the port from `aud`, and `WebPushBuilder::build`
ties `exp` to the `TTL`.

Each Web Push carries `Content-Encoding: aes128gcm`,
`Content-Type: application/octet-stream` and
`Authorization: vapid t=…, k=…`, where `k` is the public half of the
VAPID Key. A `Topic` is 1 to 32 characters of the URL-safe base64
alphabet, and the sender refuses every other. The client waits at most
10 s for each send, follows no redirect and uses no proxy.

No text message from Pagis to the Person is a notification channel.

### A Push Subscription belongs to one Session

A **Push Subscription** holds the push endpoint of one client, its P-256
public key and its auth secret. It belongs to one Session, and it ends
when that Session ends. A push service that answers `404` or `410` to a
send ends it too.

The row references its Session with `ON DELETE CASCADE`. A sign-out, a
removal from the Sessions list and the expiry sweep delete the Session
row, and the database deletes its Push Subscriptions, so no sweep of its
own is needed. The endpoint is unique. A client that subscribes again
lands on its own row, and a known endpoint that another Session sends
moves to that Session with the new keys.

A client posts the body of `PushSubscription.toJSON()` to
`POST /api/v1/push-subscriptions`. The route refuses with `422`, and a
message for each case:

- an endpoint that is not `https`, that names an IP address and not a
  DNS name, or that is longer than 2048 characters;
- a `p256dh` that is not the base64url of an uncompressed P-256 point;
- an `auth` that is not the base64url of 16 bytes.

The Person lists their Push Subscriptions, each named by the client of
its Session, with the one of the asking Session marked, and removes one.
A Push Subscription of another Person reads as absent.

### One VAPID Key for each installation

The **VAPID Key** is one P-256 key pair for each installation. It signs
each Web Push of the installation. The private half is one named entry of
`secrets.enc`, through `SecretStore` (ADR-0013), and not a Credential of
the Vault. The daemon makes it at the first need. The entry is
`vapid_private_key`: the 32-byte secret key as base64url. It goes in
through the create-if-absent operation of the store, so two first needs
agree on one key. `GET /api/v1/push/key` answers the public half as the
base64url of the uncompressed point, the `applicationServerKey` that
`PushManager.subscribe` takes.

### The payload is a Declarative Web Push message

The plaintext of each Web Push is the WebKit Declarative Web Push JSON:

- `"web_push": 8030`.
- A `notification` member with `title`, `body`, `navigate` and `data`.
  `title` is the name of the Agent of the item, or "Pagis" for an item
  of no Agent. `body` is the line of the item, and for an Approval the
  title of the Approval on a second line. `navigate` is the place of the
  item on the Public Origin.
- `"app_badge"`: the count of the Needs-You Queue, as a number. WebKit
  reads it from the top level of the message, not from `notification`.
- `"mutable": true`.

The daemon cuts `body` at a word boundary and ends it with an ellipsis
when the plaintext would be over 2048 bytes.

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

### Outcomes

The daemon sends each Notification to each Push Subscription in a task
of its own, and reads the `Outcome`:

- `Delivered` keeps the time of the send in `last_sent_at`.
- `Gone` deletes the Push Subscription.
- `RateLimited` sends once more after the delay of `Retry-After`, at
  most 300 s later, or 300 s later when the push service names no
  delay.
- Every other `Outcome` writes a log line with the id of the Push
  Subscription and the status, and never the payload.

`POST /api/v1/push-subscriptions/{push_subscription_id}/test` sends a
Notification of kind `test`, with the title "Pagis", the body
"Notifications work here." and `navigate` on `/settings/notifications`,
to one Push Subscription of the Person. It sets no badge. The route
answers the `Outcome`, and acts on `Delivered` and `Gone` in the same
way. A Push Subscription of another Person reads as absent.

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

The daemon posts only to an `https` endpoint with a DNS name. The DNS
resolver of the sender's HTTP client resolves the name at the time of the
send and keeps only the addresses that `is_public_unicast` of
`pagis-core` accepts (ADR-0029). A name with no such address fails, and
nothing goes. The client connects to an address that the resolver
answered, and does not resolve the name a second time. A name that moves
to a private address after the registration therefore gets nothing.

A test sender has the policy `AllowLoopback`, which also takes `http` and
the loopback addresses, so a test reaches a push service on its own
machine.

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
- The relay holds one transport for each platform that it serves. A
  registration for a platform with no transport gets `422`.

A server posts a Web Push to `POST /v1/push/<id>`. The relay does these
checks in this order:

1. An unknown id gets `404`.
2. A `Content-Encoding` that is not `aes128gcm` gets `415`.
3. No `Authorization: vapid t=…, k=…` gets `401` with
   `WWW-Authenticate: vapid` (RFC 8292).
4. A VAPID token that is not valid gets `403`. The token is valid only
   when `k` is the VAPID Key of the registration, the JWT header is
   `{"typ": "JWT", "alg": "ES256"}` or `{"alg": "ES256"}`, the signature
   verifies with `k`, `aud` is the origin of the relay, and `exp` is in
   the future and at most 24 hours ahead. One pure function checks it
   with `p256`, `base64` and `serde_json`. The relay links no JWT crate.
5. A body over 2800 bytes gets `413`. The relay reads the body with this
   limit, so it never holds a larger body. RFC 8030 asks a push service
   to take 4096 bytes, but the envelopes of APNs and FCM cannot hold that
   after base64, and a server sends at most 2800 bytes.
6. No `TTL`, a `TTL` that is not a number, an `Urgency` outside
   `very-low`, `low`, `normal` and `high`, or a `Topic` that is not 1 to
   32 URL-safe base64 characters gets `400`. No `Urgency` is `normal`.
7. A push over 1000 on one registration in one UTC day gets `429`, with
   `Retry-After` until the next UTC midnight. The columns `pushes_today`
   and `day` hold the count, so a restart keeps it.

The relay then gives the body, unchanged, and the `TTL`, the `Urgency`
and the `Topic` to the transport of the platform. The transport answers:

- Delivered: `201 Created` with
  `Location: <origin>/v1/messages/<random id>` and the `TTL` of the
  request (RFC 8030 section 5). The relay keeps the time in
  `last_push_at`. It stores no message, so the `Location` gets `404`.
- Gone: the relay removes the registration and answers `410`, and
  `pagis-push` reports the subscription as gone.
- Failed: `502`. The push does not count against the day.

`pagis-push` takes `401` and `403` as a failure, and not as a gone
subscription.

`PUSH_RELAY_PUBLIC_ORIGIN` is an `https` origin, or an `http` origin on
a loopback address, which a browser also takes as a secure context. A
test sender with the policy `AllowLoopback` reaches a relay on its own
machine through it, and the `aud` of its token is the relay's origin.

A log line holds the route, the id and the status, and for a push the
`Urgency` and the size of the body. It never holds a device token, a
secret, a VAPID Key, a VAPID token or a body.

### The APNs transport

The relay sends to APNs with its own small client on `reqwest`, with its
`http2` feature, and on `p256`. `apns-h2` is not used, because it needs
`aws-lc-rs` or OpenSSL and the workspace uses ring. `a2` has had no
release since 2024. The client is one request and one JWT, and the relay
verifies ES256 with the same `p256` crate.

The relay serves `ios` when these four variables are set:

- `PUSH_RELAY_APNS_KEY_PATH`: the `.p8` file of the APNs key;
- `PUSH_RELAY_APNS_KEY_ID`;
- `PUSH_RELAY_APNS_TEAM_ID`;
- `PUSH_RELAY_APNS_TOPIC`: the bundle id, `app.pagis.mobile` (ADR-0032).

With none of them, the relay serves no `ios` registration. With some of
them, the relay stops at start with a message that names each missing
variable. A key file that is not a P-256 private key in PKCS#8 PEM also
stops it.

The provider token is an ES256 JWT with `alg` and `kid` (the key id) in
its header, and `iss` (the team id) and `iat` in its claims. APNs refuses
a token older than one hour, and a new token more often than once in 20
minutes. So the relay keeps one token for 50 minutes and then makes a new
one. After a `403` with `ExpiredProviderToken` or `InvalidProviderToken`,
the relay drops the token, and the next push makes a new one.

Each push is one `POST /3/device/<device token>` over HTTP/2, to
`https://api.push.apple.com` or `https://api.sandbox.push.apple.com`, as
the APNs environment of the registration says. The body is:

```json
{"aps": {"alert": {"title": "Pagis", "body": "Something needs you"},
         "mutable-content": 1, "sound": "default"},
 "p": "<the push body as unpadded base64url>"}
```

The relay cannot read the content, so the alert is a fixed placeholder.
`mutable-content` starts the Notification Service Extension of the
Mobile App, which decrypts `p` and replaces the placeholder with the
decrypted text. iOS shows the placeholder when the extension fails. A
push body of 2800 bytes gives a JSON of less than the 4096 bytes that
APNs takes.

The headers of the request are:

- `authorization: bearer <provider token>`;
- `apns-push-type: alert`;
- `apns-topic` from the settings;
- `apns-priority: 10` for `Urgency: high`, and `5` for each other
  urgency;
- `apns-expiration`: the time of the forward plus the `TTL`, in seconds
  since the epoch;
- `apns-collapse-id`: the `Topic`, when the push has one.

The answer of APNs gives the delivery:

- `200`: Delivered.
- `410`, or `400` with the reason `BadDeviceToken` or
  `DeviceTokenNotForTopic`: Gone.
- Each other answer, and no answer: Failed. The reason of the failure
  holds the status and the APNs `reason`, and never the device token.

### The FCM transport

The relay sends to FCM with the HTTP v1 API, on `reqwest`. The legacy FCM
API is shut down. The `gcp_auth` crate gives the OAuth 2 access token of
a service account for the scope
`https://www.googleapis.com/auth/firebase.messaging`. It reads the JSON
key of the service account, keeps the token, and makes a new one before
it expires. Its crypto is ring, so it brings no `aws-lc-rs`. It reads the
root certificates of the system, so the host of the relay must have them.
The transport reads the token through a small trait, so a test gives a
fixed token.

The relay serves `android` when these two variables are set:

- `PUSH_RELAY_FCM_CREDENTIALS_PATH`: the JSON key of the service account;
- `PUSH_RELAY_FCM_PROJECT_ID`: the Firebase project of the Mobile App.

With none of them, the relay serves no `android` registration. With one
of them, the relay stops at start with a message that names the missing
variable. A key file that does not parse also stops it.

Each push is one
`POST https://fcm.googleapis.com/v1/projects/<project>/messages:send`
with the access token as a bearer token. The body is a data message:

```json
{"message": {"token": "<device token>",
             "data": {"p": "<the push body as unpadded base64url>"},
             "android": {"priority": "HIGH", "ttl": "86400s",
                         "collapse_key": "<Topic>"}}}
```

- `priority` is `HIGH` for `Urgency: high`, and `NORMAL` for each other
  urgency.
- `ttl` is the `TTL` in seconds, at most the four weeks that FCM takes.
- `collapse_key` is the `Topic`, and it is present only when the push
  has one.
- The message has no `notification` member. The Android messaging
  service of the Mobile App decrypts the body and shows the
  Notification.

FCM takes at most 4096 bytes of data, keys and values. A push body of
2800 bytes is 3734 characters of unpadded base64url, so the data fits.

The answer of FCM gives the delivery. The error code is the `errorCode`
of the `FcmError` detail of the error, or else its `status`.

- `200`: Delivered.
- `404` with the error code `UNREGISTERED`: Gone.
- `400` with `INVALID_ARGUMENT` and a `google.rpc.BadRequest` field
  violation on `message.token`: Gone. FCM sends the same code for a bad
  payload, and a fault in the relay must not remove every registration,
  so `INVALID_ARGUMENT` on another field is not Gone.
- Each other answer, no answer, and no access token: Failed. The reason
  of the failure holds the status and the error code, and never the
  device token.

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

- The hold while active, and the `activity` frame.
- The service worker of the Product App.
- The Mobile App (ADR-0032).
