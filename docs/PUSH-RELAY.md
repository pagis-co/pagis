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

The Push Relay holds the APNs keys and the FCM credentials of the
publisher. An installation of the Mobile App registers with the relay
and gets a Web Push endpoint. The server of the Person sends to that
endpoint as it sends to the push service of a browser. The relay checks
the push and forwards the ciphertext to APNs or FCM.

The store apps use the relay that the project runs. A person who builds
the Mobile App with their own APNs keys and Firebase project deploys a
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
Behind the Cloudflare Tunnel of the deployment, that entry is the
address that connected to Cloudflare (see
[Deploy the relay](#deploy-the-relay)).
An IPv6 client counts by its /64 network: all the addresses in one /64
share one window, because one site usually gets a /64 and can use each
address in it. An IPv4 client counts by its full address. An
IPv4-mapped IPv6 address counts as its IPv4 address.

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
| `PUSH_RELAY_TRUSTED_PROXY` | The IP address of the reverse proxy. The relay reads `X-Forwarded-For` only from this address. Without it, the relay believes no `X-Forwarded-For`. | `compose.yaml`: `10.231.0.2`, the address of the tunnel |
| `PUSH_RELAY_APNS_PRODUCTION_KEY_PATH` | The `.p8` file of the APNs key of the production environment: a P-256 private key in PKCS#8 PEM. | The secret `apns-production-key` |
| `PUSH_RELAY_APNS_PRODUCTION_KEY_ID` | The 10-character ID of the APNs key of the production environment. | `.env` |
| `PUSH_RELAY_APNS_SANDBOX_KEY_PATH` | The `.p8` file of the APNs key of the sandbox environment: a P-256 private key in PKCS#8 PEM. | The secret `apns-sandbox-key` |
| `PUSH_RELAY_APNS_SANDBOX_KEY_ID` | The 10-character ID of the APNs key of the sandbox environment. | `.env` |
| `PUSH_RELAY_APNS_TEAM_ID` | The 10-character ID of the Apple developer team that holds the APNs keys. | `.env` |
| `PUSH_RELAY_APNS_TOPIC` | The bundle ID of the Mobile App, `co.pagis.mobile`. | `.env` |
| `PUSH_RELAY_FCM_CREDENTIALS_PATH` | The JSON key of the Google service account that sends. | `compose.fcm.yaml`: the secret `fcm-credentials` |
| `PUSH_RELAY_FCM_PROJECT_ID` | The ID of the Firebase project of the Mobile App. | `compose.fcm.yaml`: `.env` |
| `RUST_LOG` | The filter of the log lines. The default is `info`. | Not set |

The relay serves `ios` registrations of an APNs environment when the key
pair of that environment (its `_KEY_PATH` and its `_KEY_ID`) is set,
together with `PUSH_RELAY_APNS_TEAM_ID` and `PUSH_RELAY_APNS_TOPIC`. A
relay can serve the production environment, the sandbox environment, or
both. A registration for an environment without a key gets `422`, and
the message names the platform and the environment. The relay serves
`android` registrations when the two `PUSH_RELAY_FCM_*` variables are
set.

With none of the variables of a platform, the relay serves no
registration of that platform. With half of a key pair, or some but not
all of the FCM variables, the relay stops and names each missing
variable. A team ID or a topic without a key pair also stops it. The
deployment in `deploy/push-relay/` serves both APNs environments, so it
requires each APNs variable. It serves `android` only with
`compose.fcm.yaml`, which requires the two FCM variables.

Each APNs key is Topic Specific to the bundle ID of the Mobile App, and
the relay has one key for each environment. Apple lets a key for both
environments be Team Scoped only, so such a key can send to each app of
the team. With a Topic Specific key for each environment, a key that
leaks from the relay can send only to the Mobile App, and only in the
environment of that key. The production environment serves the
TestFlight and App Store builds, and the sandbox environment serves the
debug builds.

The relay reads the key files when it starts. A key file that it cannot
read or parse stops it.

## Deploy the relay

`deploy/push-relay/` holds the deployment: the relay, and a Cloudflare
Tunnel (`cloudflared`) in front of it. The tunnel connects out to
Cloudflare, and Cloudflare holds the TLS certificate of the name of the
relay. No service publishes a port, so the host needs no inbound port and
no public IP address.

The two services share a network with the fixed subnet `10.231.0.0/24`.
The tunnel has the fixed address `10.231.0.2`, and the relay believes
`X-Forwarded-For` from that address alone. The Cloudflare edge puts the
address that connected to it at the end of `X-Forwarded-For`, and it
sets the header to that address when the request has none
([Cloudflare HTTP headers](https://developers.cloudflare.com/fundamentals/reference/http-headers/#x-forwarded-for)).
cloudflared does not change the header. So the last entry is the address
of the phone or the server, the same address as `CF-Connecting-IP`, and
the registration limit counts that address.

`compose.yaml` serves the iOS app through APNs. `compose.fcm.yaml` adds
the Android app through FCM. `COMPOSE_FILE` in `.env` selects the files:
`compose.yaml` alone, or `compose.yaml:compose.fcm.yaml` to serve the
Android app too. Without `compose.fcm.yaml`, the relay serves no
`android` registration, and the deployment needs no Firebase project.

You need:

- a Linux host with Docker Engine and the Compose plugin. The host
  connects out to Cloudflare on port 7844 (TCP and UDP), and to APNs and
  FCM on port 443;
- a domain on Cloudflare DNS, such as `pagis.co`, and a Cloudflare
  account that can make a Cloudflare Tunnel;
- two APNs keys of the Apple developer team that publishes the Mobile
  App, each Topic Specific to the bundle ID of the Mobile App: a key for
  the sandbox environment and a key for the production environment. Keep
  the `.p8` file of each key;
- for the Android app only: a JSON key of a service account of the
  Firebase project of the Mobile App that can send with the FCM HTTP v1
  API.

Do these steps in the Cloudflare dashboard:

1. Go to **Networking** > **Tunnels**, and select **Create a tunnel**.
   Give the tunnel a name, such as `pagis-push-relay`.
2. The dashboard shows an installation command for the cloudflared
   connector. Copy the token of the tunnel from it: the long value that
   starts with `eyJ`. Do not run the command. The deployment runs
   cloudflared.
3. On the **Routes** tab of the tunnel, select **Add route** >
   **Published application**. Set the subdomain (`push`) and the domain
   (`pagis.co`). The name is `PUSH_RELAY_DOMAIN`. Set the service URL to
   `http://relay:8080`. cloudflared runs on the network of the
   deployment, and the DNS of Docker on that network resolves the
   service name `relay`. Cloudflare adds the DNS record of the name.
4. Make sure that Cloudflare does not challenge or block a request to
   the name. The Mobile App and each server send `POST` requests with no
   browser. They cannot solve a challenge, so a challenge stops each
   registration and each push:
   - **Bot Fight Mode** challenges API and mobile app traffic, and a
     rule cannot skip it for one name. Turn it off in **Security** >
     **Settings** for the zone. On a plan with Super Bot Fight Mode, add
     a WAF custom rule that skips it for the name instead.
   - **Browser Integrity Check** challenges a request with no user agent
     or with a user agent that is not a browser. Add a configuration
     rule (**Rules** > **Configuration Rules**) for the hostname of the
     relay that turns Browser Integrity Check off.
   - Keep **I'm Under Attack** mode off for the name, and put no
     Cloudflare Access application on it.
   - Keep the managed transform **Remove visitor IP headers** off, and
     keep **Pseudo IPv4** off. Each of them changes the address that
     the relay counts.

   **Security** > **Events** shows each request that Cloudflare
   challenged or blocked, and the feature that did it.

Do these steps on the host:

1. Copy `deploy/push-relay/` from the tag of the relay version that you
   deploy.
2. Copy `.env.example` to `.env`, and set each value in it. To serve the
   Android app, set `COMPOSE_FILE=compose.yaml:compose.fcm.yaml`.
3. Put the APNs key files and the token of the tunnel in `secrets/`.
   Paste the token, and then push Ctrl-D, so that the token is not in
   the history of the shell:

   ```bash
   mkdir -p secrets
   cp /path/to/AuthKey_ABC123DEFG.p8 secrets/apns-production-key.p8
   cp /path/to/AuthKey_GHI456JKLM.p8 secrets/apns-sandbox-key.p8
   cat > secrets/cloudflared-token
   ```

4. Give each file to the user of the service that reads it, and make it
   private. Compose mounts each file with its owner and its mode on the
   host. The relay runs as the user ID 10001, and cloudflared runs as
   the user ID 65532:

   ```bash
   sudo chown 10001:10001 secrets/apns-production-key.p8 \
     secrets/apns-sandbox-key.p8
   sudo chown 65532:65532 secrets/cloudflared-token
   sudo chmod 400 secrets/apns-production-key.p8 \
     secrets/apns-sandbox-key.p8 secrets/cloudflared-token
   ```

5. Only with `compose.fcm.yaml`: put the JSON key of the service account
   at `secrets/fcm-credentials.json`, and give it to the user of the
   relay:

   ```bash
   cp /path/to/service-account.json secrets/fcm-credentials.json
   sudo chown 10001:10001 secrets/fcm-credentials.json
   sudo chmod 400 secrets/fcm-credentials.json
   ```

6. Start the deployment. Compose reads `COMPOSE_FILE` from `.env`:

   ```bash
   docker compose up -d
   ```

7. Make sure that the relay is healthy. `docker compose ps` shows the
   relay as `healthy`, the Cloudflare dashboard shows the tunnel as
   `Healthy`, and the health route answers the version:

   ```bash
   curl https://push.example.net/v1/health
   ```

The named volume `relay-data` holds the state directory and the SQLite
file.

To upgrade, set `PUSH_RELAY_VERSION` in `.env` to the new version, and
take the `compose.yaml` and the `compose.fcm.yaml` of its tag. Then run
`docker compose pull` and `docker compose up -d`. The relay migrates its
database when it starts.

## Rotate the keys

The relay reads the key files only when it starts, so a new key takes
effect when Compose makes the relay container again. A registration does
not change when a key changes: a device token stays valid with each key
of the team or of the project.

Rotate the APNs key of each environment apart from the other. To rotate
the key of one environment:

1. In the Apple developer account, make a new APNs key for the same
   environment, Topic Specific to the bundle ID of the Mobile App. Keep
   the old key.
2. Put the `.p8` file of the new key at `secrets/apns-production-key.p8`
   for the production environment, or at `secrets/apns-sandbox-key.p8`
   for the sandbox environment, with the owner 10001 and the mode 400.
3. Set `PUSH_RELAY_APNS_PRODUCTION_KEY_ID` or
   `PUSH_RELAY_APNS_SANDBOX_KEY_ID` in `.env` to the ID of the new key.
4. Make the relay container again:

   ```bash
   docker compose up -d --force-recreate relay
   ```

5. Make sure that the relay is healthy and that its log shows no APNs
   failure.
6. Revoke the old key in the Apple developer account.

To rotate the FCM credentials of a deployment with `compose.fcm.yaml`:

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
