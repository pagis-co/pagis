# mobile/

The source of the Pagis Mobile App, a Capacitor 8 app for iOS and Android.
The app bundles only the Connect screen (`index.html`) and the Unreachable
screen (`unreachable.html`), with their code in `src/`. It opens the Product
App of the server that the Person chooses, and it shows the Unreachable
screen when that server does not load. The pages of the
[Mobile App](https://docs.pagis.co/mobile-app/connect-to-a-server) in the
documentation site (`docs-site/content/mobile-app/`) state what the app does
for the people who use it.
[ADR-0032](../docs/adr/0032-the-mobile-app-is-a-native-shell-around-the-product-app.md)
holds the decisions. This document tells how to build, test and run it.

## Toolchain

- Node.js at the version in [`.nvmrc`](../.nvmrc).
- For Android: a JDK 21, and the Android SDK 36 in the directory that
  `ANDROID_HOME` names.
- For iOS: Xcode 26 or later on macOS, with an iPhone simulator.

The checks skip a native test whose toolchain is absent. `cargo xtask step`
fails it, so a CI job that names the step never passes a test that did not
run.

## Development

```bash
npm ci
npm run typecheck
npm test        # the vitest tests of src/
npm run build   # the bundled screens, into dist/
```

`npx cap sync` copies `dist/` and the plugin list into the native projects.
Run it after each build, and after a change to the Capacitor plugins in
`package.json`:

```bash
npx cap sync android
npx cap sync ios
```

## App icons

The app uses the Pagis mark on a white ground. iOS has one opaque 1024 px
icon for iPhone, iPad and the App Store. Android has square and round
icons at each density, adaptive foreground layers inside the 66 dp safe
circle, and a monochrome layer for themed icons on Android 13 and later.
The launcher cuts the adaptive icon to its own shape.

`scripts/draw-app-icons.mjs` uses the shared mark renderer in
[`assets/brand/mark.mjs`](../assets/brand/mark.mjs). To make the PNG files
again:

```bash
npm run icons
```

The native projects hold the generated files. `src/appIcons.test.ts`
checks their sizes, colors, transparency and safe margins, and the native
resource references.

## Native tests

The JUnit tests are in `android/app/src/test/` and the XCTest tests are in
`ios/App/AppTests/`.

```bash
(cd android && ./gradlew testDebugUnitTest)
(cd ios && xcodebuild test -project App/App.xcodeproj -scheme App \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro')
```

The gate runs the same tests:

| Step | What it does |
| --- | --- |
| `mobile-deps` | `npm ci` |
| `mobile-typecheck`, `mobile-test` | The npm scripts |
| `mobile-android-test` | `npm run build`, `npx cap sync android`, then `./gradlew testDebugUnitTest` |
| `mobile-ios-test` | `npm run build`, `npx cap sync ios`, then `xcodebuild test` on the first available iPhone simulator |

## Push

The app registers its Push Subscription through the Push Relay
([ADR-0032](../docs/adr/0032-the-mobile-app-is-a-native-shell-around-the-product-app.md),
[the Push Relay guide](../docs/PUSH-RELAY.md)). Two values of the build
name the relay and its Firebase project. The repository holds a
placeholder for each, so a build is the same as a production build, but
no push arrives:

- `PUSH_RELAY_ORIGIN`: the `https` origin of the Push Relay. The
  placeholder is `https://push-relay.invalid`, a reserved name that never
  resolves.
  - iOS: the build setting `PUSH_RELAY_ORIGIN` of the `App` target in
    `ios/App/App.xcodeproj`. `Info.plist` gives it to the app as
    `PagisPushRelayOrigin`. Change the value in **Build Settings** in
    Xcode for each configuration, or give it to one build:
    `xcodebuild PUSH_RELAY_ORIGIN=https://<relay host> ...`.
  - Android: the `buildConfigField` `PUSH_RELAY_ORIGIN` in
    `android/app/build.gradle`.
- `android/app/google-services.json`: the Firebase project of the Push
  Relay. In the Firebase console, add an Android app with the package name
  `co.pagis.mobile` to the project that `PUSH_RELAY_FCM_PROJECT_ID` of the
  relay names. Download its `google-services.json`, and put it in place of
  the placeholder.

On iOS, the entitlement `aps-environment` lets the app get an APNs token.
A debug build registers the APNs environment `sandbox` with the relay, and
a release build registers `production`. The APNs key of the relay must
belong to the team that signs the app.

The app embeds the Notification Service Extension `PagisNotificationService`
(`ios/App/PagisNotificationService/`), which decrypts each push and shows
it. The extension compiles some sources of `ios/App/App/` too: the target
membership of a file in Xcode shows which. The app and the extension share
two groups, which the entitlements of both targets name:

- the Keychain access group `<team id>.co.pagis.mobile`, which holds the
  keys of the Push Subscription;
- the App Group `group.co.pagis.mobile`, whose `UserDefaults` hold the
  server origin.

To sign for a device, register the App Group in the Apple Developer
account, and turn on App Groups with it for the App IDs
`co.pagis.mobile` and `co.pagis.mobile.PagisNotificationService`. A
simulator build needs no team.

To check the extension on a phone, run the `App` scheme on the phone, turn
on Notifications in the app, and make a Request on the server. To debug
the extension, select **Debug → Attach to Process by PID or Name** in
Xcode, type `PagisNotificationService`, and send a push.

`fixtures/web-push.json` at the repository root holds the keys of a Push
Subscription, one body that `pagis-push` encrypts for them, and its
plaintext. The XCTest tests read a copy of it in the test bundle.
`PAGIS_PUSH_WRITE_FIXTURE=1 cargo nextest run -p pagis-push --run-ignored only`
makes it again.

On Android, `PagisMessagingService` decrypts each push and shows it. The
JUnit tests read `fixtures/web-push.json` at the path that the test task
gives in the system property `pagis.webPushFixture`. To check the service
on a phone, run the app on the phone with the `google-services.json` of
the Firebase project of the relay, turn on Notifications in the app, and
make a Request on the server. `adb logcat -s Pagis` shows why a push shows
"Something needs you" in place of its text, and why the daemon did not take
an answer from **Approve once** or **Deny**.

## Run the app against a local daemon

A debug build of the app takes `http://` on a loopback host. A release build
takes `https://` only.

1. Start a daemon from the repository root:

   ```bash
   (cd ui && npm ci && npm run build)
   cargo run -p pagis -- --local
   ```

   The daemon answers on `http://127.0.0.1:4400`. At the first run it prints
   a one-time sign-in link. `pagis pair` prints a new one.

2. Build the Connect screen and sync it:

   ```bash
   npm run build
   npx cap sync
   ```

3. Open the app on a simulator or an emulator:

   - iOS: `npx cap run ios`, or open `ios/App/App.xcodeproj` in Xcode and
     run the `App` scheme on an iPhone simulator. The simulator shares the
     loopback of the Mac, so `127.0.0.1:4400` is the daemon.
   - Android: start an emulator, forward the port of the daemon to it, then
     run the app:

     ```bash
     adb reverse tcp:4400 tcp:4400
     npx cap run android
     ```

4. On the Connect screen, type `http://127.0.0.1:4400`, or paste the sign-in
   link that the daemon printed.

To change the server, touch and hold the app icon and select **Change
server**.
