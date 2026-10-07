# The Push Relay image. The relay forwards a Web Push to APNs and FCM for
# the store apps of the Mobile App (ADR-0030). `docs/PUSH-RELAY.md` holds
# the deployment and the release.
#
# The build context is the repository root, because the relay is a member
# of the workspace and builds with its lockfile:
#
#   docker buildx build -f crates/pagis-push-relay/Dockerfile \
#     -t ghcr.io/pagis-co/pagis-push-relay:<version> .
#   cargo xtask relay-image
#
# The bases are the pins of the root `Dockerfile`, by tag and digest
# (`cargo xtask pins --check`). Dependabot opens the pull requests that
# move the digests.

FROM rust:1-trixie@sha256:a8a5f0a1e5fe7dfe1d352591e4a1c7dd2c08fd70475cae872cf3458ba0df0546 AS build
# cargo-auditable writes the list of the crates of the relay into the
# executable, so Trivy identifies them in the image. `pagis-versions`
# pins the version, and the other images use the same one.
ARG CARGO_AUDITABLE_VERSION=0.7.6
RUN cargo install --locked --version "$CARGO_AUDITABLE_VERSION" cargo-auditable
WORKDIR /src
COPY . .
ENV CARGO_INCREMENTAL=0
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    set -eux; \
    cargo auditable build --release -p pagis-push-relay; \
    cp target/release/pagis-push-relay /usr/local/bin/pagis-push-relay

FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a
# ca-certificates: the TLS connections to APNs, to FCM and to the token
#   endpoint of Google, which `gcp_auth` verifies with the root
#   certificates of the system.
# curl: the container health check, and nothing the relay starts.
# The upgrade takes each Debian security fix that is out at build time,
# also before Docker publishes a base image that holds it, so the
# vulnerability scan of a release does not stop on a fixed package of the
# base.
RUN apt-get update && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# The relay runs as a user of its own, with a fixed ID, so a deployment
# can give the key files to that ID. It owns the state directory and
# nothing else.
RUN groupadd --system --gid 10001 pagis-push-relay \
    && useradd --system --uid 10001 --gid pagis-push-relay \
       --home-dir /var/lib/pagis-push-relay --no-create-home \
       --shell /usr/sbin/nologin pagis-push-relay \
    && mkdir -p /var/lib/pagis-push-relay \
    && chown pagis-push-relay:pagis-push-relay /var/lib/pagis-push-relay \
    && chmod 700 /var/lib/pagis-push-relay

COPY --from=build --chown=root:root /usr/local/bin/pagis-push-relay /usr/local/bin/pagis-push-relay
COPY LICENSE /usr/local/share/doc/pagis-push-relay/LICENSE

# The state directory holds the SQLite file of the registrations. A
# deployment keeps a volume here.
ENV PUSH_RELAY_DATABASE=/var/lib/pagis-push-relay/relay.sqlite
VOLUME /var/lib/pagis-push-relay

# The relay terminates no TLS. A reverse proxy in front of it holds the
# certificate and forwards to this port.
ENV PUSH_RELAY_BIND=0.0.0.0:8080
EXPOSE 8080

# The port is a setting, so the check reads it from `PUSH_RELAY_BIND`.
HEALTHCHECK --interval=15s --timeout=5s --start-period=10s --retries=5 \
    CMD curl -fsS "http://127.0.0.1:${PUSH_RELAY_BIND##*:}/v1/health" || exit 1

# The version in this image, which `cargo xtask relay-image` checks
# against the version of the crate before it pushes.
LABEL co.pagis.push-relay.version="0.1.0"
# GHCR reads this to link the package to the repository it was built
# from, which is where a reader goes for the source and the license.
LABEL org.opencontainers.image.source="https://github.com/pagis-co/pagis"

USER 10001:10001
ENTRYPOINT ["pagis-push-relay"]
