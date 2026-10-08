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
identifier is `co.pagis.mobile` on both platforms, after the project
domain `pagis.co`. The first store release makes it permanent.

### The bridge serves the main frame of the server's exact origin only

The bridge answers a call only from the main frame of the server's exact
origin: the scheme, the host and the port must all match. A Widget frame
and every other origin get no bridge. The native side checks the frame
and the origin of each bridge call, and it does not trust the page to do
this check. A link to every other origin opens in the system browser, as
in the product window of the Client App (ADR-0025).

- On iOS, a guard takes the place of the `bridge` message handler of
  Capacitor and of its UI delegate. It passes a message, or a `prompt()`
  that Capacitor reads for its cookie and HTTP calls, only from the main
  frame of the origin that the bridge shows.
- On Android, Capacitor makes the check with the web message listener of
  the Android System WebView, from the server URL as a bare origin. With
  no such listener, every frame reaches the legacy bridge of Capacitor. So
  on a web view with no web message listener the app makes no bridge, and
  it asks the Person to update the Android System WebView.
- Only the exact origin that the bridge shows stays in the web view. A
  main-frame navigation, or a new window, to each other `http` or `https`
  origin opens in the system browser, also another port or another scheme
  of the same host. Capacitor alone keeps a URL that starts with the text
  of the server URL on iOS, and a URL of the same scheme and host on
  Android, so the shell makes this check in front of Capacitor on both
  platforms. `allowNavigation` stays empty.

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

- On iOS, in a Keychain item with `kSecAttrAccessibleAfterFirstUnlock`, in
  the Keychain access group `<team>.co.pagis.mobile`, which the
  Notification Service Extension shares.
- On Android, in a file that a Tink AEAD encrypts. The keyset comes from
  `AndroidKeysetManager`, with a master key in the Android Keystore. Tink
  is also the library of the messaging service, and the deprecated Jetpack
  Security library is not used.

A native request sends the copy as a `Cookie` header, with no `Origin` and
no `Sec-Fetch-Site`. The cross-origin check of the daemon
(`crates/pagis-server/src/cross_origin.rs`) passes such a request to the
Session check as a request from a program. The Host socket of the Client
App goes through the check in the same way.

The copy follows the web view. On iOS a `WKHTTPCookieStoreObserver` reads
the cookie at each change of the store. Android has no such event, so the
shell reads `CookieManager` after each main-frame page load and when the
activity pauses, and flushes the store when it pauses. `CookieManager` gives
no expiry, so on Android the copy lasts 30 days from its last read, which
is the life of a Session from its last use. A new Session replaces the
copy, and a removed cookie deletes it.

At launch, when the cookie store holds no Session cookie of the server and
the copy is live, the shell writes the copy back into the store before the
first load, with the attributes that the daemon gives the cookie. So a
Session that native requests kept alive stays signed in in the web view
too.

When a Session ends, the Product App calls `PagisShell.sessionEnded()`
through the bridge. The shell deletes the copy, forgets the server and
opens the Connect screen. **Change server** also deletes the copy. A native
request that gets a `401` deletes the copy.

### Push goes through the Push Relay

The Product App registers no service worker where `window.Capacitor` is
present (`ui/src/push/register.ts`). WKWebView gives no service worker to
a remote origin without App-Bound Domains, and the Android System WebView
has no Push API. So the native side receives each Notification.

The plugin `PagisPush` of the app target has `state()`,
`subscribe({vapidKey})` and `unsubscribe()`. Where
`Capacitor.isNativePlatform()` is true, the Notifications section of
Settings uses it in place of `PushManager`. `subscribe` does these steps
in this order:

1. It asks the phone for the permission to show notifications:
   `UNUserNotificationCenter.requestAuthorization` on iOS, and
   `POST_NOTIFICATIONS` on Android 13 and later.
2. It gets the token: an APNs token from
   `registerForRemoteNotifications()` on iOS, or an FCM token from
   `FirebaseMessaging` on Android.
3. It registers the token and the VAPID Key of the server with the Push
   Relay, which gives the token a Web Push endpoint (ADR-0030). A debug
   build on iOS registers the APNs environment `sandbox`, and a release
   build `production`. The origin of the relay is the build constant
   `PUSH_RELAY_ORIGIN`.
4. It makes the keys of the subscription (RFC 8291): a P-256 key pair
   and 16 random bytes of auth secret.
5. It answers the shape of `PushSubscription.toJSON()`: the endpoint of
   the relay, the 65-byte uncompressed public point as `p256dh`, and the
   auth secret, both as base64url with no padding.

The Product App posts the answer to the daemon through the route that a
browser uses. So the Push Subscription belongs to the Session of the
Mobile App.

The app keeps the registration (its id, secret and endpoint, the VAPID
Key and the token) and the keys beside the copy of the Session: in the
Keychain access group with `kSecAttrAccessibleAfterFirstUnlock` on iOS,
and in files that the Tink AEAD of the app encrypts on Android. The
private key never leaves native code.

- A second `subscribe` with the same VAPID Key registers nothing, and it
  answers the stored values. A `subscribe` with another VAPID Key deletes
  the earlier registration and its keys first.
- A new token goes to the relay with `PUT`, so the endpoint and the Push
  Subscription of the daemon stay the same. On iOS the app asks for the
  token at each launch, and it sends the token only when it changed. On
  Android `PagisMessagingService` gets a new token in `onNewToken`. When
  the relay does not know the registration, the app forgets it and its
  keys, and the next `subscribe` registers again.
- To turn Notifications off, the Product App deletes the Push
  Subscription of the daemon, and then calls `unsubscribe()`. That
  deletes the registration with the relay, and then the keys.
- A new Session has no Push Subscription. The section then shows
  Notifications as off, and **Turn on** uses the registration that the
  app holds.

The app does not use `@capacitor/push-notifications`. That plugin owns the
Android `FirebaseMessagingService` of the app, and on iOS it handles a
push through the `NotificationRouter` of Capacitor, which calls the
completion handler at once. The decryption, the tap and the inline answer
need both of these paths, so the app owns them:

- On iOS, `ios.handleApplicationNotifications` is `false` in
  `capacitor.config.json`, and `NotificationResponder` of the app is the
  delegate of `UNUserNotificationCenter`.
- On Android, `PagisMessagingService` is the `FirebaseMessagingService`
  of the app. `google-services.json` names the Firebase project of the
  Push Relay.

The iOS Notification Service Extension decrypts the payload with CryptoKit.
The Android messaging service decrypts it with the `apps-webpush` module of
Tink. The Push Relay, APNs and FCM see only ciphertext.

### The Notification Service Extension shows a Notification on iOS

The app embeds the Notification Service Extension `PagisNotificationService`.
APNs starts it for each push of the Push Relay, because the push has
`mutable-content` (ADR-0030). The extension has about 30 seconds and a small
memory limit. When it fails or runs out of time, iOS shows the placeholder
of the relay.

The app and the extension share these sources, and the extension has no
copy of them:

- `PushKeys.swift`: the keys of the Push Subscription. The extension reads
  the keys and never makes them.
- `WebPushDecrypt.swift`: `decrypt(body:privateKey:auth:)`, one pure
  function after RFC 8291 and RFC 8188. It reads the salt, the record size
  and the key of the sender from the header, makes the ECDH secret with
  `P256.KeyAgreement`, makes the key and the nonce of the content with
  `HKDF<SHA256>`, and opens the record with AES-128-GCM. The body is one
  record that ends with the delimiter `0x02` and zero bytes of padding. A
  short body, a bad header, a second record, a record that does not open
  and another delimiter are each an error.
- `PushPayload.swift`: the parser of the JSON of ADR-0030. A field of the
  wrong type is an error. An unknown field is ignored.
- `NotificationContent.swift`: the content of the Notification.
- `ServerStore.swift`: the origin of the server, in the `UserDefaults` of
  the App Group `group.co.pagis.mobile`.

The extension has the Keychain access group and the App Group of the app.
For each push, it decodes `p`, decrypts it with the keys in the Keychain,
and parses the payload. The Notification then has:

- the `title` and the `body` of the payload;
- the `kind` as its thread identifier, so iOS groups the Notifications of
  one kind;
- `app_badge` as the badge, when the payload has one;
- `navigate`, `item`, `kind` and `request` in its `userInfo`;
- the category `approval` when the `actions` of `request` are
  `approve_once` and `deny`, and no category for each other payload.

When the app holds no keys, when the push does not decrypt or does not
parse, and when `data.v` is not `1`, the extension delivers the
placeholder of the relay as it is, with the server origin as `navigate`.
When iOS ends the time of the extension, it delivers the same placeholder.

The file `fixtures/web-push.json` holds the keys of a Push Subscription,
one body that `pagis-push` encrypted for them, and its plaintext. A test
of `pagis-push` decrypts the body with `web-push-native`, and the XCTest
tests of the app decrypt it with `decrypt`. So the sender and the
clients agree on one encryption.

### The messaging service shows a Notification on Android

The Push Relay sends FCM a data message `{"p": "<body>"}`, with the
priority `HIGH` for `Urgency: high` and `NORMAL` for each other urgency
(ADR-0030). Android shows nothing for a data message, so
`onMessageReceived` of `PagisMessagingService` shows the Notification.
FCM can lower the priority of an app whose `HIGH` messages show no
notification, so each push shows one.

For each push, the service reads the keys of the Push Subscription from
the files that the Tink AEAD of the app encrypts, decodes `p`, decrypts
it, and parses the payload:

- `WebPushDecrypt.java` decrypts the body with `WebPushHybridDecrypt` of
  the `apps-webpush` module of Tink. The app takes that module without
  its dependency `tink`, because `tink-android` holds the same classes.
  `pagis-push` writes the length of the one record as the record size of
  the header, but Tink takes only the record size of its builder, and it
  counts the header in that size. The record size only marks where a
  record ends, and the encryption does not cover it. So the app checks
  that the body is one record, and gives Tink a copy with the record size
  4096. A short body, a second record, a record that does not open and a
  padding delimiter other than `0x02` are each an error.
- `PushPayload.java` parses the JSON of ADR-0030 with the rules of
  `PushPayload.swift`. A field of the wrong type is an error. An unknown
  field is ignored.

`PushNotifier.java` then shows the Notification:

- the channel `needs_you` ("Needs you", high importance) for the kinds
  that the daemon sends with `Urgency: high` (`approval`, `waiting` and
  `keypad`), and the channel `activity` ("Activity", default importance)
  for each other kind;
- the `title` and the `body` of the payload;
- the `kind` as its group, and the `item` as its tag, so a new push for
  the same item replaces the old Notification;
- `app_badge` as its number, for a launcher that shows a count;
- a content intent that opens `MainActivity` with the extra `navigate`.

When the app holds no keys, when the push does not decrypt or does not
parse, and when `data.v` is not `1`, the service shows "Pagis" and
"Something needs you", with the server origin as `navigate`. Its channel
is `needs_you` for a push of the priority `HIGH`, and `activity` for each
other push. A new fallback Notification replaces the old one.

The JUnit tests decrypt the vector of RFC 8291 Appendix A and
`fixtures/web-push.json` with `WebPushDecrypt`. The test task gives the
path of the fixture in the system property `pagis.webPushFixture`.

### Inline answers are Approve once and Deny

A Notification of a pending tool action or credential action Approval
shows **Approve once** and **Deny** when this phone has **Answer on the lock screen** on. The per-device setting is off by default and the You screen changes it. With it off, iOS registers the approval category without actions and Android draws no actions. These are the `actions` of the payload (ADR-0030).

- **Approve once** posts `{"decision": "approved"}` to the decision route,
  `POST /api/v1/requests/{request_id}/decision`, with no `scope`. The
  route reads no `scope` as `once`.
- **Deny** posts `{"decision": "denied"}` to the same route.

The app never sends `scope: "always"`, so an answer from a Notification
never writes an Allow Rule.

When the answer fails for any reason, the app shows one Notification:
"Pagis did not take this answer. Open Pagis to see the request." A failure
is no network, a `401`, a `404`, a `409` for a Request that is already
decided, or no answer in 20 seconds, as in the service worker (ADR-0030).

On iOS, the app registers the category `approval` at launch, with the
actions `approve_once` and `deny`. Each action has `.authenticationRequired`,
**Deny** also has `.destructive`, and neither action has `.foreground`. So
iOS runs the app in the background, with no scene and no bridge, and
`NotificationResponder` gives the action to `InlineAnswer`:

1. It starts a background task with `UIApplication.beginBackgroundTask`.
2. `ApprovalAnswer` posts the decision with the header
   `Cookie: pagis_session=<copy>`, and with no `Origin` and no `scope`.
3. A `2xx` answer removes the Notification. A failure replaces the
   Notification with one that has the same identifier, title and thread,
   no actions, the text above, and the `navigate` of the item. A `401`
   also deletes the copy of the Session.
4. It calls the completion handler of the delegate, after which iOS can
   suspend the app, and then ends the background task. When iOS ends the
   time of the app first, the task ends at once.

On Android, `PushNotifier` adds **Approve once** and **Deny** to a
Notification whose `request` has the actions `approve_once` and `deny`.
Each action is an immutable broadcast `PendingIntent` to the receiver
`ApprovalReceiver`, with the Request id, the item, the kind, the title and
the `navigate` of the Notification. Each action has
`setAuthenticationRequired(true)` on Android 12 and later. Neither action
opens the app.

1. `ApprovalReceiver` gives the answer to WorkManager as the unique work
   of the Request, with `ExistingWorkPolicy.KEEP`. So a second touch, also
   on the other action, posts nothing while the first answer waits. The
   work is expedited with `OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST`,
   and it waits for a network.
2. `ApprovalWorker` reads the server and the copy of the Session, and
   `ApprovalAnswer` posts the decision with the header
   `Cookie: pagis_session=<copy>`, with no `Origin` and no `scope`, and
   with a 20 second limit.
3. A `2xx` answer removes the Notification. When no answer comes,
   WorkManager tries again after its backoff, three attempts at most. A
   `401`, a `404`, a `409`, each other status, no copy of the Session and
   the last failed attempt replace the Notification with one that has the
   same tag, title, channel and group, no actions, the text above, and the
   `navigate` of the item. A `401` also deletes the copy of the Session.

`ApprovalAnswer` sends the post with OkHttp, which has no cookie store.
The `HttpURLConnection` of Android is not used for it, because it reads
the default `CookieHandler` of the process. Capacitor sets that handler to
the cookie store of the web view, so the request would get a second
`Cookie` header, and the web view would get each cookie of the answer.

Before Android 12, WorkManager runs expedited work in a foreground
service. That service shows "Pagis sends your answer." in the channel
`answers` ("Answers", low importance), which makes no sound.

The app keeps no record of the answer. The daemon records the decision
(ADR-0004).

A tap on a Notification of every other Request (a form, a choice, a
Widget answer or a question) opens the app at the place of the Request.

An answer needs no step-up sign-in. The Session is the authority, as on
every client. Each action needs an unlocked phone (`.authenticationRequired`
on iOS, and `setAuthenticationRequired(true)` on Android 12 and later), so
a person who holds a locked phone answers nothing. Android 11 and earlier
have no such setting: there, an action runs from the lock screen when the
lock screen shows the content of the Notification.

### A tap opens the place of a Notification

A Notification holds `navigate`: the absolute URL of the place of its item
on the Public Origin. On iOS it is in the `userInfo`. On Android it is the
extra `navigate` of the content intent. A Notification that the app posts
itself, such as the failure of an inline answer, holds `navigate` too, and
the same rule applies to it.

One rule decides the place. `mobile/src/navigate.ts` holds it, and
`WebOrigin.place(of:)` on iOS and `ServerOrigin.place` on Android apply
it, with the same test cases. The app opens a `navigate` URL only when its
origin is the stored origin of the server: the scheme, the host and the
port. The app then opens the path, the query and the fragment of the URL.
Each other value opens `/`: a URL of another origin, a `javascript:` URL,
and a value that is not a URL. So a Notification never takes the web view
to another origin.

- A tap on a cold start starts the bridge at `<origin><place>` in place of
  `<origin>/`. On iOS the scene reads the tap from its connection options,
  and the next bridge opens the place as its first page. The delegate of
  the notification center then gets the same tap, and the app opens it one
  time only. On Android `MainActivity` gives the place to `CapConfig` as
  the start path. An intent that Android gives again when the Person opens
  the app from the recent apps is no new tap.
- A tap while the web view is open sends the Product App the event
  `navigate` of `PagisShell`, with `{path}`. No page loads again. The
  event waits for its first listener, so a tap that comes before the
  Product App listens is not lost. Where `Capacitor.isNativePlatform()` is
  true, the Product App listens for the event (`useShellNavigation` in
  `ui/src/mobileShell.ts`) and gives the path to its router. A browser adds
  no listener. The event does in the Mobile App what the `navigate`
  message of the service worker does in a browser (ADR-0030).

### The app clears stale Notifications when it opens

The daemon sends no dismissal push (ADR-0030). So when the app comes to
the foreground with a stored server and a copy of the Session, it reads
`GET /api/v1/needs-you` with the copy, from native code:

1. It lists its delivered Notifications first. A Notification that
   arrives during the read is not in the list, so it stays, because its
   item can be newer than the answer.
2. It removes each listed Notification whose `item` is not an item of the
   queue, also one with no `item`, such as the placeholder. iOS uses
   `removeDeliveredNotifications(withIdentifiers:)`. Android uses
   `NotificationManager.cancel(tag, id)`, where the tag is the item.
3. On iOS it sets the badge to the count of the queue with
   `setBadgeCount(_:)`. On Android the launcher shows the count of the
   Notifications that stay, so the app sets no badge.

A `401` deletes the copy of the Session, as each native request does.
Each other failure changes nothing: no network, another status, or an
answer that is not a queue. On iOS the scene starts the clean-up in
`sceneWillEnterForeground`. On Android `MainActivity` starts it in
`onResume`, off the main thread.

### The phone is not a Host

The Mobile App opens no Host socket, and no host action runs on a phone
(ADR-0015). A host command for a Person who uses only the Mobile App
answers that no computer is connected.

### The live screen, Listen-Live and Dictation run in the web view

The live screen, Listen-Live and Dictation run in the web view, as in a
browser. The live screen reaches the phone through the TURN server of
Remote Access (ADR-0028), and needs no other transport. The shell grants
the microphone only to the main frame of the server's origin, as the Client
App does (`desktop/src/webPermissions.ts`). A Widget frame or a page of
another origin gets no microphone, and no page gets the camera.

- On iOS, Capacitor grants each request of each frame. So the shell puts
  `MediaGuard` in front of the UI delegate. It decides
  `requestMediaCapturePermissionFor`: it grants `.microphone` to the main
  frame of the stored origin, and denies every other request. WebKit then
  shows the Person the system prompt alone, one time.
- On Android, Capacitor grants each requested resource, also the camera,
  after the permission of the system. So the shell sets `PagisChromeClient`
  on the web view. It grants `RESOURCE_AUDIO_CAPTURE` to the stored origin
  alone, after the app holds `RECORD_AUDIO`, and asks for `RECORD_AUDIO` at
  the first request. A request of the Android web view names an origin and
  no frame.
- On iOS, the audio session category is `.playAndRecord` with
  `.defaultToSpeaker` while the app is active, so Listen-Live plays with
  the ring/silent switch on.

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

The Product App draws the phone layout for the Mobile App (ADR-0034).

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

- The lower bound on the server version is 0.2.0, not the first release
  that serves Notifications.
