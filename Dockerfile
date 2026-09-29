# The headless Linux server image: the fourth release artifact.
#
# The client-installed server is a signed macOS disk image (ADR-0025).
# This is the same daemon for a team's own VM, where nobody sits at the
# machine: one image, one process, Postgres beside it, and a reverse
# proxy in front. `docs/RELEASING-SERVER.md` holds the release matrix and
# https://docs.pagis.co/server holds the deployment.
#
#   docker buildx build -t ghcr.io/pagis-co/pagis-server:<version> .
#   cargo xtask server-image
#
# The image carries every program the daemon starts: `git` for memory
# repacks and git Plugin installs, `gog` for Google Connections, and
# `pg_dump` and `pg_restore` for `pagis backup` and `pagis restore`.
#
# A tag can move to other code, so each base image is pinned by tag and
# digest (`cargo xtask pins --check`). Dependabot opens the pull requests
# that move the digests.

# The Product App and the Administration Interface are two entry points
# of one Vite package, and a release build embeds `ui/dist` in the
# binary, so the bundle is built before cargo runs.
FROM node:26-slim@sha256:ec7758ee051e457b468b32bde57b0879010b325bb9862718e9615225ce4aaae1 AS ui
WORKDIR /src/ui
COPY ui/package.json ui/package-lock.json ./
RUN npm ci
# The avatar catalogue, the portraits and the model the Agent pages
# import. They live above `ui/`, so the bundle needs both directories.
COPY assets/ /src/assets/
COPY ui/ ./
RUN npm run build

FROM rust:1-trixie@sha256:a8a5f0a1e5fe7dfe1d352591e4a1c7dd2c08fd70475cae872cf3458ba0df0546 AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config cmake libssl-dev \
    && rm -rf /var/lib/apt/lists/*
# cargo-auditable writes the list of the crates of `pagis` into the
# executable, so Trivy identifies them in the image. `pagis-versions`
# pins the version, and the Computer Image uses the same one.
ARG CARGO_AUDITABLE_VERSION=0.7.6
RUN cargo install --locked --version "$CARGO_AUDITABLE_VERSION" cargo-auditable
WORKDIR /src
COPY . .
COPY --from=ui /src/ui/dist ui/dist
# The Computer image the daemon pulls when an Agent wakes. A release
# passes the immutable digest it resolved; a plain build keeps the
# pinned tag of `pagis-versions`.
ARG PAGIS_COMPUTER_IMAGE=""
ENV CARGO_INCREMENTAL=0
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    set -eux; \
    if [ -z "$PAGIS_COMPUTER_IMAGE" ]; then unset PAGIS_COMPUTER_IMAGE; fi; \
    cargo auditable build --release -p pagis; \
    cp target/release/pagis /usr/local/bin/pagis

# `gog` is the Google Connection runner, and the daemon starts it as a
# sibling of its own executable (ADR-0024). The release downloads the
# same pinned build and checks the same hash.
FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a AS gog
ARG TARGETARCH
ARG GOG_VERSION=0.42.0
ARG GOG_SHA256_amd64=1967a962a57d689958c408dd0abc784792c3712da9d0a90650bb76ab7e3de388
ARG GOG_SHA256_arm64=84ce3002acea162596068c8b25e364aade634d204ae6122b714e686ca783b028
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
RUN set -eux; \
    case "${TARGETARCH:-$(dpkg --print-architecture)}" in \
      amd64) platform=linux_amd64; sha="$GOG_SHA256_amd64" ;; \
      arm64) platform=linux_arm64; sha="$GOG_SHA256_arm64" ;; \
      *) echo "no gog build for ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL "https://github.com/openclaw/gogcli/releases/download/v${GOG_VERSION}/gogcli_${GOG_VERSION}_${platform}.tar.gz" -o /tmp/gog.tar.gz; \
    echo "${sha}  /tmp/gog.tar.gz" | sha256sum -c -; \
    mkdir -p /out; \
    tar -xzf /tmp/gog.tar.gz -C /out ./gog; \
    chmod 755 /out/gog

FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a
# ca-certificates: the model providers and every other HTTPS call.
# git: best-effort memory repacks and git Plugin installs.
# postgresql-client-18: pg_dump and pg_restore, which `pagis backup` and
#   `pagis restore` run. Its major version matches the Postgres the
#   deployment runs, because pg_dump refuses a newer server. Debian
#   carries an older major, so it comes from the PostgreSQL apt
#   repository, which `postgresql-common` adds with the signing key it
#   ships.
# curl: the container health check, and nothing the daemon starts.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates git curl postgresql-common \
    && /usr/share/postgresql-common/pgdg/apt.postgresql.org.sh -y \
    && apt-get install -y --no-install-recommends postgresql-client-18 \
    && rm -rf /var/lib/apt/lists/*

# The daemon starts `gog` from its own directory, so the two live
# together and `/usr/local/bin/pagis` is a link to the one below.
COPY --from=build --chown=root:root /usr/local/bin/pagis /usr/local/lib/pagis/pagis
COPY --from=gog --chown=root:root /out/gog /usr/local/lib/pagis/gog
COPY LICENSE /usr/local/lib/pagis/LICENSE
COPY third_party/gog/LICENSE /usr/local/lib/pagis/LICENSE.gog
COPY third_party/THIRD_PARTY_NOTICES /usr/local/lib/pagis/THIRD_PARTY_NOTICES
RUN ln -s /usr/local/lib/pagis/pagis /usr/local/bin/pagis

# The state directory: the sealed secrets, the memory repositories, the
# artifacts, the recordings, the Plugins and the Software. A deployment
# keeps a volume here, and Postgres holds the records.
ENV PAGIS_HOME=/var/lib/pagis
RUN mkdir -p /var/lib/pagis
VOLUME /var/lib/pagis

# This image is always a server: people reach it from other machines,
# so it holds no Client Credential. The daemon refuses to start with
# `--local`, until `PAGIS_PUBLIC_ORIGIN` names the address people open
# it at, and, as every server does, until `PAGIS_DATABASE_URL` names its
# Postgres database.
ENV PAGIS_REQUIRE_PUBLIC_ORIGIN=true

# The product port and the administration port. The Media Relay
# range is UDP and a deployment publishes it separately, because its
# size is a setting.
EXPOSE 4400 4401

# The product port is a setting, so the check reads it. `PAGIS_PORT` is
# unset on an image a deployment leaves alone, and the default matches
# the daemon's own.
ENV PAGIS_PORT=4400
HEALTHCHECK --interval=15s --timeout=5s --start-period=60s --retries=5 \
    CMD curl -fsS "http://127.0.0.1:${PAGIS_PORT}/api/v1/health" || exit 1

# The release in this image, which `cargo xtask server-image` checks
# against the workspace version before it pushes.
LABEL org.pagis.server.version="0.1.0"
# GHCR reads this to link the package to the repository it was built
# from, which is where a reader goes for the source and the license.
LABEL org.opencontainers.image.source="https://github.com/pagis-co/pagis"

# Nobody sits at this machine, so the daemon opens no browser.
ENTRYPOINT ["pagis"]
CMD ["--no-open"]
