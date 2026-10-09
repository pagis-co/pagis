# Release the Mobile App

The iOS app of the Mobile App ships through TestFlight and the App Store
from its own tag, `mobile-v<version>`, as the Push Relay ships from
`push-relay-v<version>` (`docs/PUSH-RELAY.md`). It is not an artifact of
the `v*` release, and the matrix of `docs/RELEASING-SERVER.md` does not
name it. App Store review takes days, and a self-hosted server updates on
its own schedule, so the two releases do not wait for each other.

The Android release is not built.

## The version

- `version` in `mobile/package.json` is the version of the app, its
  `CFBundleShortVersionString`. The tag names it: `mobile-v0.2.0` for
  `0.2.0`. A tag that names another version stops the release.
- The build number, `CFBundleVersion`, is the number of the workflow run
  (`GITHUB_RUN_NUMBER`). App Store Connect takes each build number one
  time for a version. A re-run of a job keeps the number of its run, so
  after an upload that went through, a new build needs a new version.
- `MINIMUM_SERVER_VERSION` in `mobile/src/serverVersion.ts` is the oldest
  server release that the app accepts: the first release that sends
  Notifications and serves the decision route (ADR-0032). The app accepts
  each newer server, so a store app keeps working when the Administrator
  updates the server.

To release:

1. Change `version` in `mobile/package.json`, and let npm write it to
   `mobile/package-lock.json` (`npm install --package-lock-only` in
   `mobile/`).
2. Merge the change. Then push the tag on its commit:

   ```bash
   git tag mobile-v0.2.0
   git push origin mobile-v0.2.0
   ```

## The release workflow

The tag starts `.github/workflows/mobile-release.yml`. Its release has two
phases, as the release of the Client App has (`docs/RELEASING-CLIENT.md`),
so the bytes that App Store Connect gets are the bytes that were signed
and checked:

1. **The gate.** The CI workflow runs on the tagged commit. The job
   `mobile-npm-audit` then runs `cargo xtask advisories mobile-npm-audit`,
   which checks the npm lockfile of the app against the published
   advisories. A red result stops the release.
2. **Prepare.** The job `mobile-ios` runs on `macos-latest` with Xcode 26
   or later, and runs:

   ```bash
   cargo xtask mobile --ios --tag mobile-v0.2.0 --prepare
   ```

   The command checks its inputs, then:
   1. builds the Connect screen and runs `npx cap sync ios`;
   2. archives the app and the Notification Service Extension with
      `xcodebuild archive`. The command line sets `MARKETING_VERSION` to
      the version, `CURRENT_PROJECT_VERSION` to the run number, and
      `PUSH_RELAY_ORIGIN` to the environment variable of the same name. A
      setting on the command line comes before each xcconfig file, so the
      placeholder of `mobile/ios/app.xcconfig` does not apply;
   3. exports `mobile/release/Pagis.ipa` with
      `mobile/ios/ExportOptions.plist`: the method `app-store-connect`,
      automatic signing, and the versions of the archive;
   4. checks the exact `.ipa`: the signature of the app and of the
      extension, their versions and privacy manifests, the bundle id
      `co.pagis.mobile`, the relay origin, and the APNs environment
      `production`.

   The job attests the provenance of the `.ipa` and keeps it as the
   artifact `mobile-ios` of the run.
3. **Publish.** The job `publish-mobile-ios` waits in the `release`
   environment until a maintainer approves it. It downloads the artifact
   and runs:

   ```bash
   cargo xtask mobile --ios --tag mobile-v0.2.0 --publish-existing
   ```

   The command uploads the prepared `.ipa` to App Store Connect with
   `xcrun altool`. It does not build or sign again.

The job `mobile-ios` sets `PUSH_RELAY_ORIGIN` to `https://push.pagis.co`,
the production Push Relay. The prepare phase refuses a value that is not
an `https` origin, and the placeholder `https://push-relay.invalid`.

`--dry-run` prints the plan. The plan names its inputs, and holds no
secret value: the shell reads each secret from the environment when a
step runs.

### Signing

The release uses Xcode automatic signing with the App Store Connect API
key: `-allowProvisioningUpdates`, `-authenticationKeyPath`,
`-authenticationKeyID` and `-authenticationKeyIssuerID`. Xcode makes the
profiles of the app and of the extension in the team of
`DEVELOPMENT_TEAM` in `project.pbxproj`. The export signs with a
cloud-managed distribution certificate, so no certificate or private key
is a secret of the repository.

The archive signs with an Apple Development certificate first. A runner
starts with an empty keychain, so Xcode makes a new development
certificate on each run. When the account reaches the limit of
development certificates, revoke the old certificates of the runners in
**Certificates, Identifiers & Profiles**.

The entitlement `aps-environment` is `development` in
`mobile/ios/App/App/App.entitlements`. The distribution profile sets it
to `production` in the exported app, and the prepare phase refuses an
`.ipa` with another value. The APNs key of the Push Relay must belong to
the same team (`docs/PUSH-RELAY.md`).

### The inputs

| Name | Where | What it holds |
| --- | --- | --- |
| `APPLE_API_KEY_P8` | Repository secret | The text of the `.p8` file of an App Store Connect API key with the Admin role |
| `APPLE_API_KEY_ID` | Repository secret | The Key ID of that key |
| `APPLE_API_ISSUER` | Repository secret | The Issuer ID of the team |
| `PUSH_RELAY_ORIGIN` | The workflow | `https://push.pagis.co` |
| `GITHUB_RUN_NUMBER` | GitHub Actions | The build number |

These are the same three API key secrets that the `v*` release uses to
notarize. `.github/scripts/write-notary-key.sh` writes the key to a file
of the runner, and names the file in `APPLE_API_KEY`.

The key needs the **Admin** role. Automatic signing makes certificates,
App IDs and profiles, and a cloud-managed distribution certificate takes
the Admin role. A key with the Developer role notarizes, but Xcode refuses
it for cloud signing. `scripts/signing-secrets.sh` walks the account
holder through a key with the Admin role.

## The Apple Developer account

Before the first release, the account holder or an Admin of the team does
these steps one time:

1. In **Certificates, Identifiers & Profiles → Identifiers**, register the
   App Group `group.co.pagis.mobile`.
2. Register the App ID `co.pagis.mobile` with these capabilities:
   - **App Groups**, with `group.co.pagis.mobile`;
   - **Push Notifications**.
3. Register the App ID `co.pagis.mobile.PagisNotificationService` with
   **App Groups**, with `group.co.pagis.mobile`.
4. In App Store Connect, add the app: **Apps → New App**, the platform
   iOS, the name Pagis, the bundle ID `co.pagis.mobile`, and a SKU. The
   first upload needs this record.
5. Make an API key with the Admin role, and set the three secrets
   (`scripts/signing-secrets.sh`).
6. Make the maintainers the required reviewers of the `release`
   environment of the repository.

The Keychain access group `<team id>.co.pagis.mobile` needs no
registration: it starts with the App ID prefix of the team. The app and
the extension keep the keys of the Push Subscription in it.

## TestFlight and App Store review

App Store Connect processes an upload before it shows the build under
**TestFlight**. Then:

1. Answer the export compliance question of the build. The app uses the
   encryption of iOS only: HTTPS, and the decryption of a Web Push with
   CryptoKit.
2. Add the build to an internal testing group. Each tester installs it
   with the TestFlight app on an iPhone or an iPad.
3. Before each submission, install the build on an iPhone and on an iPad.
   On the iPhone, sign in with a scanned Sign-In Link, get a Notification
   from a test daemon through the production Push Relay, and select
   **Approve once** on the lock screen. The Run continues.
4. On the page of the version under **App Store**, select the build, fill
   in the App Privacy answers, the screenshots and the review notes below,
   then select **Add for Review** and **Submit for Review**.

App Store review is a manual step after the upload. The workflow does not
submit a build for review.

### The App Privacy answers

The answer is **Data Not Collected**:

- The app sends no analytics and does no tracking. Its privacy manifests
  set `NSPrivacyTracking` to `false` and name no collected data type.
- The server address and the Session stay on the phone and on the server
  of the Person. The project does not run that server.
- The Push Relay holds the push token and the relay registration, only to
  deliver a Notification (`docs/PUSH-RELAY.md`, "What the relay stores").
  The relay never sees the text of a Notification.

Check these answers against the definitions of "collected" data in the
App Privacy details of App Store Connect before each submission.

The privacy manifests are `mobile/ios/App/App/PrivacyInfo.xcprivacy` and
`mobile/ios/App/PagisNotificationService/PrivacyInfo.xcprivacy`. Each one
gives the reason `1C8F.1` for `UserDefaults`, because the app and the
extension keep the server origin in the `UserDefaults` of the App Group.
Capacitor and Cordova ship their own manifests. The plugins App, Browser
and Barcode Scanner, and the OSBarcodeLib library of the scanner, ship
none and call no required-reason API. `mobile/src/privacyManifests.test.ts`
checks the manifests and the Capacitor plugins.

### The screenshots

The app is universal, so App Store Connect asks for screenshots of an
iPhone and of an iPad: the largest iPhone display (6.9 inch) and the
largest iPad display (13 inch). Take them on the simulators of those
displays with a server that holds demo data: the Connect screen, the
Needs-You queue, a Request, and a Notification with **Approve once**.

### The review notes

App Store review guideline 4.2 refuses an app that is only a website
(ADR-0032). The review notes name the native parts of the app:

- the sign-in with a scanned Sign-In Link;
- Notifications that the phone decrypts;
- the answers **Approve once** and **Deny** from the lock screen.

The reviewer needs a server. Run a demo server with Remote Access, and
give in the notes:

- a fresh Sign-In Link of an invite for each submission, as a link to
  paste and as a QR code. The link of an invite is good for seven days
  (CONTEXT.md, "Sign-In Link"), so make it on the day of the submission;
- the steps to get a Notification: the action on the demo server that
  makes a Request.

## The steps that only a maintainer can do

- The account steps of [The Apple Developer account](#the-apple-developer-account).
- The approval of the `release` environment for each release.
- The export compliance answer, the TestFlight testers, the check on an
  iPhone and an iPad, and the submission for App Store review.
- The demo server, the Sign-In Link and the QR code of each submission.
- The revocation of old development certificates when the account reaches
  its limit.

## Not built

- The Android release.
- The App Store listing. The documentation site names no App Store link.
