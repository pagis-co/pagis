# The Push Relay

The Push Relay forwards a Web Push to APNs and FCM for the store apps of
the Mobile App. This document states what the relay stores and sees, its
limits and settings, and how to deploy, operate and release it. ADR-0030
holds the decisions.

## What the relay is for

A server sends each Notification as a standard Web Push. A browser gets
the push from the push service of the browser. A Mobile App from the App
Store or Google Play cannot get a Web Push in this way. APNs and FCM
accept a push only with a key of the publisher of the app, and a
self-hosted server does not hold that key.

The Push Relay holds the APNs key and the FCM credentials of the
publisher. An installation of the Mobile App registers with the relay
and gets a Web Push endpoint. The server of the Person sends to that
endpoint as it sends to the push service of a browser. The relay checks
the push and forwards the ciphertext to APNs or FCM.

The store apps use the relay that the project runs. A person who builds
the Mobile App with their own APNs key and Firebase project deploys a
relay of their own with this guide.

A build of the Mobile App names its relay in the build constant
`PUSH_RELAY_ORIGIN`, and the Firebase project in
`mobile/android/app/google-services.json`. `mobile/README.md` tells how to
set them.

The relay is the crate `crates/pagis-push-relay`, a library and the
binary `pagis-push-relay`. It is not an artifact of the Release Matrix.
No installation runs it, and its routes start with `/v1/` and stay
compatible. So it has its own version and its own tag
(see [Release the relay](#release-the-relay)).

## What the relay stores

The relay keeps one row for each registration in its SQLite file:

| Field | What it holds |
| --- | --- |
| The id | The random id in the endpoint. The endpoint holds no device token, so a leaked endpoint names no phone. |
| The hash of the secret | The SHA-256 of the secret that the installation sends to change or remove its registration. The relay cannot give the secret back. |
| The platform | `ios` with its APNs environment (`production` or `sandbox`), or `android`. |
| The device token | The APNs or FCM token of the installation. |
| The VAPID Key | The public half of the VAPID Key of the server of the Person. Each push to the endpoint must be signed with it. |
| The counts | The number of pushes in the UTC day, and that day. |
| The times | The time of the registration, and the time of the last push that APNs or FCM took. |

The relay keeps no other record. It stores no push, no payload and no
message. The registration limit counts in memory, so a restart of the
relay forgets those counts.

A log line holds the route, the id and the status, and for a push the
`Urgency` and the size of the body. A log line never holds a device
token, a secret, a VAPID Key, a VAPID token or a body.

## What the relay never sees

The relay never sees the content of a Notification. The server encrypts
each payload with the keys of the Push Subscription of the phone
(RFC 8291), and only the phone decrypts it.

For each push, the relay sees:

- the ciphertext and its size;
- the time of the push;
- the `TTL`, the `Urgency` and the `Topic` headers;
- the VAPID token of the server. Its `sub` claim is the Public Origin of
  the server when that origin is `https`;
- the network address of the server.

APNs gets a fixed alert, "Pagis" and "Something needs you", and the
ciphertext. The Notification Service Extension of the Mobile App
decrypts the ciphertext and replaces the alert. FCM gets a data message
that holds only the ciphertext.

## The limits

| Limit | Value | What a request above the limit gets |
| --- | --- | --- |
| Registrations from one client address | 20 in each hour | `429` with `Retry-After` until the end of the hour of that address |
| Pushes to one registration | 1000 in each UTC day | `429` with `Retry-After` until the next UTC midnight |
| The body of one push | 2800 bytes | `413` |

The registration limit counts in a fixed window of one hour for each
address. Behind a reverse proxy, the address is the last entry of
`X-Forwarded-For` from the address that `PUSH_RELAY_TRUSTED_PROXY` names.

The database holds the count of pushes, so a restart keeps it. A push
that APNs or FCM does not take gets `502` and does not count against the
day.

A server sends at most 2800 bytes, so that the ciphertext fits the 4096
bytes of APNs and FCM after base64 and their envelopes.

## The settings

The relay reads its settings from environment variables when it starts.
A setting that is not valid stops the relay with a message that names
the variable.

| Variable | What it holds | In `deploy/push-relay/` |
| --- | --- | --- |
| `PUSH_RELAY_PUBLIC_ORIGIN` | Required. The `https` origin of the relay, such as `https://push.example.net`. Each endpoint starts with it, and the `aud` of each VAPID token must be it. An `http` origin on a loopback address is accepted for tests. | `https://` and `PUSH_RELAY_DOMAIN` |
| `PUSH_RELAY_DATABASE` | Required. The path of the SQLite file. The relay makes the file and its tables when they are missing. | The image: `/var/lib/pagis-push-relay/relay.sqlite` |
| `PUSH_RELAY_BIND` | The IP address and the port that the relay listens on. The default is `127.0.0.1:8080`. | The image: `0.0.0.0:8080` |
| `PUSH_RELAY_TRUSTED_PROXY` | The IP address of the reverse proxy. The relay reads `X-Forwarded-For` only from this address. Without it, the relay believes no `X-Forwarded-For`. | `compose.yaml`: `10.231.0.2`, the address of Caddy |
| `PUSH_RELAY_APNS_KEY_PATH` | The `.p8` file of the APNs key: a P-256 private key in PKCS#8 PEM. | The secret `apns-key` |
| `PUSH_RELAY_APNS_KEY_ID` | The 10-character ID of the APNs key. | `.env` |
| `PUSH_RELAY_APNS_TEAM_ID` | The 10-character ID of the Apple developer team. | `.env` |
| `PUSH_RELAY_APNS_TOPIC` | The bundle ID of the Mobile App, `co.pagis.mobile`. | `.env` |
| `PUSH_RELAY_FCM_CREDENTIALS_PATH` | The JSON key of the Google service account that sends. | The secret `fcm-credentials` |
| `PUSH_RELAY_FCM_PROJECT_ID` | The ID of the Firebase project of the Mobile App. | `.env` |
| `RUST_LOG` | The filter of the log lines. The default is `info`. | Not set |

The relay serves `ios` registrations when the four `PUSH_RELAY_APNS_*`
variables are set, and `android` registrations when the two
`PUSH_RELAY_FCM_*` variables are set. With none of the variables of a
platform, the relay serves no registration of that platform. With some
but not all of them, the relay stops and names each missing variable.
The deployment in `deploy/push-relay/` serves both platforms, so it
requires each of them.

The relay reads the key files when it starts. A key file that it cannot
read or parse stops it.

## Deploy the relay

`deploy/push-relay/` holds the deployment: the relay and Caddy, which
holds the TLS certificate. The two services share a network with the
fixed subnet `10.231.0.0/24`. Caddy has the fixed address `10.231.0.2`,
and the relay believes `X-Forwarded-For` from that address alone.

You need:

- a Linux host with Docker Engine and the Compose plugin;
- a DNS name for the relay, with an A record to the host. Give the name
  no AAAA record: the network of the deployment is IPv4, and Docker
  forwards an IPv6 connection through its own proxy. The relay then sees
  one address for each IPv6 client, and they share one registration
  limit;
- the ports 80 and 443 open to the internet. Caddy uses port 80 to get
  its certificate;
- the `.p8` file of an APNs key of the Apple developer team that
  publishes the Mobile App;
- a JSON key of a service account of the Firebase project of the Mobile
  App that can send with the FCM HTTP v1 API.

Do these steps on the host:

1. Copy `deploy/push-relay/` from the tag of the relay version that you
   deploy.
2. Copy `.env.example` to `.env`, and set each value in it.
3. Put the two key files in `secrets/`:

   ```bash
   mkdir -p secrets
   cp /path/to/AuthKey_ABC123DEFG.p8 secrets/apns-key.p8
   cp /path/to/service-account.json secrets/fcm-credentials.json
   ```

4. Give the key files to the user of the relay, and make them private.
   Compose mounts each file with its owner and its mode on the host, and
   the relay runs as the user ID 10001:

   ```bash
   sudo chown 10001:10001 secrets/apns-key.p8 secrets/fcm-credentials.json
   sudo chmod 400 secrets/apns-key.p8 secrets/fcm-credentials.json
   ```

5. Start the deployment:

   ```bash
   docker compose up -d
   ```

6. Make sure that the relay is healthy. `docker compose ps` shows the
   relay as `healthy`, and the health route answers the version:

   ```bash
   curl https://push.example.net/v1/health
   ```

The named volume `relay-data` holds the state directory and the SQLite
file. The volumes `caddy-data` and `caddy-config` hold the certificate.

To upgrade, set `PUSH_RELAY_VERSION` in `.env` to the new version, and
take the `compose.yaml` and the `Caddyfile` of its tag. Then run
`docker compose pull` and `docker compose up -d`. The relay migrates its
database when it starts.

## Rotate the keys

The relay reads the key files only when it starts, so a new key takes
effect when Compose makes the relay container again. A registration does
not change when a key changes: a device token stays valid with each key
of the team or of the project.

To rotate the APNs key:

1. In the Apple developer account, make a new APNs key. Keep the old key.
2. Put the `.p8` file of the new key at `secrets/apns-key.p8`, with the
   owner 10001 and the mode 400.
3. Set `PUSH_RELAY_APNS_KEY_ID` in `.env` to the ID of the new key.
4. Make the relay container again:

   ```bash
   docker compose up -d --force-recreate relay
   ```

5. Make sure that the relay is healthy and that its log shows no APNs
   failure.
6. Revoke the old key in the Apple developer account.

To rotate the FCM credentials:

1. In the Google Cloud console, add a new JSON key to the service
   account. Keep the old key.
2. Put the new JSON key at `secrets/fcm-credentials.json`, with the owner
   10001 and the mode 400.
3. Make the relay container again:

   ```bash
   docker compose up -d --force-recreate relay
   ```

4. Make sure that the relay is healthy and that its log shows no FCM
   failure.
5. Delete the old key of the service account.

A restart forgets the counts of the registration limit.

## Back up the database

The database holds every registration. Without it, each endpoint that
the relay gave answers `404`, and each server removes the Push
Subscription of that endpoint. Stop the relay before the copy, so that
the SQLite file and its write-ahead log agree:

```bash
docker compose stop relay
docker run --rm -v pagis-push-relay_relay-data:/data:ro --entrypoint tar \
  ghcr.io/pagis-co/pagis-push-relay:<version> -C /data -cf - . > relay-data.tar
docker compose start relay
```

The archive holds the device tokens. Keep it private, and encrypt it with
your backup tool.

To restore the archive, stop the relay, empty the volume and unpack the
archive into it:

```bash
docker compose stop relay
docker run --rm -i -v pagis-push-relay_relay-data:/data --entrypoint sh \
  ghcr.io/pagis-co/pagis-push-relay:<version> \
  -c 'find /data -mindepth 1 -delete && tar -C /data -xf -' < relay-data.tar
docker compose start relay
```

## Release the relay

A tag `push-relay-v<version>` publishes the image
`ghcr.io/pagis-co/pagis-push-relay:<version>`. To release:

1. Change `version` in `crates/pagis-push-relay/Cargo.toml`, and let
   Cargo write it to `Cargo.lock` (`cargo check -p pagis-push-relay`).
2. Change the label `co.pagis.push-relay.version` in
   `crates/pagis-push-relay/Dockerfile` to the same version.
3. Change `PUSH_RELAY_VERSION` in `deploy/push-relay/.env.example` to the
   same version.
4. Merge the change. Then push the tag on its commit:

   ```bash
   git tag push-relay-v0.2.0
   git push origin push-relay-v0.2.0
   ```

The tag starts `.github/workflows/push-relay.yml`:

1. **The gate.** The CI workflow runs on the tagged commit, and a red
   result stops the release.
2. **The images.** One job for each architecture runs on a Linux runner
   of that architecture (`ubuntu-24.04` and `ubuntu-24.04-arm`). It runs
   `cargo xtask relay-image --platform <platform> --tag <tag>`. The job
   checks the advisories of the Cargo lockfile with cargo-deny. Then it
   builds the image, scans its filesystem for secrets with gitleaks and
   for known vulnerabilities with Trivy, and pushes it by digest only
   after both scans pass.
3. **The manifest.** `cargo xtask relay-image --manifest --tag <tag>`
   joins both digests under the version. The job attests the provenance
   of the digest.

Each `cargo xtask relay-image` refuses a tag that does not name the
crate version, and a Dockerfile whose label is not the crate version.

`cargo xtask relay-image --dry-run` prints the plan on any host. Without
`--platform` and `--manifest`, the command builds each architecture in
turn and then joins them. The push needs a Docker login to GHCR with
`write:packages`.

The first push makes a private GHCR package. Make `pagis-push-relay`
public on its package page, so that a deployment pulls it with no login.

The image builds on the same pinned bases as the Headless Server image:
`rust:1-trixie` for the build and `debian:trixie-slim` for the runtime.
`cargo xtask pins --check` holds the pins, and Dependabot moves their
digests. The runtime stage upgrades the Debian packages of its base, so
each build holds the Debian security fixes that are out. The build uses
the cargo-auditable that `pagis-versions` pins, so Trivy identifies the
crates of the relay.
