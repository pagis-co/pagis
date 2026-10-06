# 0032: The Mobile App is a native shell around the server's Product App

Status: accepted.

## Context

The phone is a pager and a remote. From the phone the Person answers
Requests, gives work, reads the Report, watches a Desk and follows a Call.
No work runs on the phone: the work runs on the Agents' Computers.

Remote Access lets a phone open the Product App through the owner's
Tailscale Funnel, and a Sign-In Link signs the phone in from a QR code
(ADR-0028). A browser on a phone does not do all that a pager does. Safari
on iOS gives Web Push only to a Home Screen web app, and it shows no
notification actions (ADR-0030). So on an iPhone a Notification cannot
carry **Approve once** and **Deny**, and a Person who does not add the
Product App to the Home Screen gets no Notification.

Products that put the interface of a server on a phone share one shape:

- Home Assistant Companion is a native app around the server's own web
  interface. It adds native notifications and a small bridge between the
  page and the phone, and it bundles no copy of the interface.
- The Client App does the same on a computer. Its product window loads the
  Product App of its server and bundles none (ADR-0025).

Home Assistant advisory GHSA-7jp2-p2fw-mgvf shows the risk of this shape.
The Companion bridge answered every frame of the web view, also a
cross-origin iframe of a Webpage card, and that iframe could take the
access token of the signed-in user.

## Decision

### A Capacitor 8 shell for iOS and Android

The **Mobile App** is a Capacitor 8 app, on the stable 8.5 line, for iOS
and Android. It loads the Product App of the server that the Person
chooses. That server is the server URL of the bridge, and the shell sets
the URL at launch:

- On iOS, `instanceDescriptor()` of a `CAPBridgeViewController` subclass
  returns a descriptor with the server URL.
- On Android, the `BridgeActivity` builds its bridge from a `CapConfig`
  with the server URL.

The session that builds the shell reads the Capacitor 8 API first, and
uses these customization points as that API names them.

The app bundles only a Connect screen. It bundles no copy of the Product
App, so the web part is always the Product App of the server. It connects
only over `https://`, so it reaches a Server, or a Local Installation in
Remote Access.

The iOS app is universal: it runs on an iPhone and on an iPad. The app
identifier is `app.pagis.mobile` on both platforms. The owner can change
it before the first store release.

### The bridge serves the main frame of the server's exact origin only

The bridge answers a call only from the main frame of the server's exact
origin: the scheme, the host and the port must all match. A Widget frame
and every other origin get no bridge. The native side checks the frame
and the origin of each bridge call, and it does not trust the page to do
this check. A link to every other origin opens in the system browser, as
in the product window of the Client App (ADR-0025).

### The Mobile App signs in with a Sign-In Link

The Connect screen scans the QR code of a Sign-In Link. It also takes a
pasted link or a server address. With a link, the web view opens the
server's own `/sign-in#<secret>` page, and that page posts the secret to
the daemon, as in the Client App (ADR-0025, ADR-0028). With an address
alone, the web view opens the server's own sign-in page. The shell sends
no secret itself.

The Session is a `browser` Session. The Mobile App adds no `ClientKind`,
no sign-in route and no new trade. The shell appends `Pagis/<version>` to
the User-Agent, through `appendedUserAgentString`. The daemon reads the
`Pagis/` token first (`crates/pagis-server/src/client_name.rs`), so the
Sessions list names the Session "Pagis on iPhone", "Pagis on iPad" or
"Pagis on Android". On an iPad, WKWebView sends the User-Agent of Safari
on macOS, so the shell also appends `iPad`. The Person removes a Session
of the Mobile App in the Sessions list, as any other Session.

### The native side keeps a copy of the Session

A Notification arrives when the web view is not open, and an answer from
a Notification goes from native code. So the native side keeps a copy of
the Session.

The shell reads the Session cookie, `pagis_session`, from the cookie store
of the web view: `WKHTTPCookieStore` on iOS and `CookieManager` on Android.
Both give an `HttpOnly` cookie to native code. The shell keeps the copy:

- On iOS, in a Keychain access group that the Notification Service
  Extension shares, with `kSecAttrAccessibleAfterFirstUnlock`.
- On Android, in storage that a key of the Android Keystore encrypts.

A native request sends the copy as a `Cookie` header, with no `Origin` and
no `Sec-Fetch-Site`. The cross-origin check of the daemon
(`crates/pagis-server/src/cross_origin.rs`) passes such a request to the
Session check as a request from a program. The Host socket of the Client
App goes through the check in the same way.

The copy follows the web view. A new Session replaces it, and a sign-out
or a `401` removes it.

### Push goes through the Push Relay

The Product App registers no service worker where `window.Capacitor` is
present (`ui/src/push/register.ts`). WKWebView gives no service worker to
a remote origin without App-Bound Domains, and the Android System WebView
has no Push API. So the native side receives each Notification.

1. The shell gets an APNs token on iOS or an FCM token on Android.
2. The Push Relay gives that token a Web Push endpoint (ADR-0030).
3. The app makes its own P-256 key pair and auth secret (RFC 8291). It
   keeps them beside the copy of the Session.
4. The app registers a Push Subscription with the daemon through the
   route that a browser uses. So the Push Subscription belongs to the
   Session of the Mobile App.

The iOS Notification Service Extension decrypts the payload with CryptoKit.
The Android messaging service decrypts it with the `apps-webpush` module of
Tink. The Push Relay, APNs and FCM see only ciphertext.

### Inline answers are Approve once and Deny

A Notification of a pending tool action or credential action Approval
shows **Approve once** and **Deny**. These are the `actions` of the
payload (ADR-0030).

- **Approve once** posts `{"decision": "approved"}` to the decision route,
  `POST /api/v1/requests/{request_id}/decision`, with no `scope`. The
  route reads no `scope` as `once`.
- **Deny** posts `{"decision": "denied"}` to the same route.

The app never sends `scope: "always"`, so an answer from a Notification
never writes an Allow Rule.

When the answer fails for any reason, the app shows one Notification:
"Pagis did not take this answer. Open Pagis to see the request." A failure
is no network, a `401`, a `409` for a Request that is already decided, or
a timeout.

A tap on a Notification of every other Request (a form, a choice, a
Widget answer or a question) opens the app at the place of the Request.

An answer needs no step-up sign-in. The Session is the authority, as on
every client. Each action needs an unlocked phone (`.authenticationRequired`
on iOS), so a person who holds a locked phone answers nothing.

### The phone is not a Host

The Mobile App opens no Host socket, and no host action runs on a phone
(ADR-0015). A host command for a Person who uses only the Mobile App
answers that no computer is connected.

### The live screen, Listen-Live and Dictation run in the web view

The live screen, Listen-Live and Dictation run in the web view, as in a
browser. The live screen reaches the phone through the TURN server of
Remote Access (ADR-0028). The shell grants the microphone only to the main
frame of the server's origin, as the Client App does
(`desktop/src/webPermissions.ts`).

### Compatibility is a lower bound

The web part is always the server's own Product App, so it always matches
the server. The native part depends only on two things: the payload format
of ADR-0030 and the decision route.

The app reads the server version from `/api/v1/health` `.version`. It
refuses a server older than the first release that serves Notifications,
with the words of the Client App (`desktop/src/serverCompatibility.ts`):
"Ask the administrator of the server to update it to <version> or newer."

A store app and a self-hosted server update at different times, so the
rule has no upper bound. The Client App holds a caret Compatibility Range
(ADR-0025). The Mobile App holds a lower bound only.

In return, the payload format, with its version `v`, and the decision route
are a stable contract (ADR-0030). This contract is the one exception to the
rule of no backward compatibility. A change that breaks the contract raises
`v`, and a Mobile App release that reads the new `v` ships first.

Other ways were considered:

- **A Home Screen web app alone.** It needs no store, but Safari on iOS
  shows no notification actions, and a Person must add the app to the Home
  Screen before any Notification arrives.
- **A native app with its own interface.** It would be a second Product
  App on two platforms, and it would need a release for each change of the
  Product App.
- **A Capacitor app that bundles the Product App.** Its interface would
  fall behind the server between releases, and it would need an upper
  bound on the versions of the server.

## Consequences

- App Store review guideline 4.2 refuses an app that is only a repackaged
  website, and this is a real risk. The native value of the Mobile App is
  the scan of a Sign-In Link, Notifications decrypted on the phone, and
  answers from the lock screen. The store release writes this in the
  review notes.
- The payload format and the decision route are a contract with the Mobile
  App releases in the stores. A server change that breaks either one needs
  a Mobile App release that reads the new form first.
- Capacitor documents the server URL for live reload during development.
  The shell sets it at launch for every start, and the gate tests that
  path.
- The Mobile App needs an `https://` server. A Local Installation reaches
  it only in Remote Access.
- The project holds an Apple Developer Program account and a Google Play
  Console account, and it runs the Push Relay.
- A lost phone keeps its Session until the Person removes it in Settings,
  under Sessions. The removal also ends its Push Subscription (ADR-0030).
- A Person who uses only the Mobile App has no Host. A host action needs
  a computer that runs the Client App.

## Not built

- The Mobile App: the shell, the Connect screen, the bridge and its origin
  check, the copy of the Session, the Push Subscription through the Push
  Relay, the Notification Service Extension, the Android messaging
  service, the inline answers and the lower bound on the server version.
