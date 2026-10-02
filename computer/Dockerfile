# The agent computer image: debian:trixie-slim with
# labwc 0.9.8 + wlroots 0.19 built from source (the trixie apt versions
# lack ext-image-copy-capture-v1), Chromium on ozone/wayland maximized,
# foot, and the pagis screen daemon. The daemon pulls this image from
# GHCR at boot or when an agent wakes, so an installation works only
# while the pinned tag is on the registry. One command builds both
# architectures and pushes it:
#
#   docker login ghcr.io
#   cargo xtask image
#
# The org.pagis.computer.version label below must equal the
# IMAGE_VERSION pin in crates/pagis-computer; the daemon refuses a
# mismatched image, and the publish refuses to start on a mismatch.
#
# A tag can move to other code, so each base image is pinned by tag and
# digest, and each source checkout by tag and commit ID. `cargo xtask
# pins --check` holds the base image rule, and Dependabot opens the pull
# requests that move the digests.

FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a AS compositor
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential meson ninja-build pkg-config git ca-certificates \
    wayland-protocols libwayland-dev libpixman-1-dev libdrm-dev \
    libxkbcommon-dev libcairo2-dev libpango1.0-dev libglib2.0-dev \
    libxml2-dev librsvg2-dev libinput-dev \
    libgbm-dev libudev-dev libseat-dev libdisplay-info-dev libliftoff-dev hwdata \
    && rm -rf /var/lib/apt/lists/*

# wlroots 0.19 — headless/pixman only: no GL/Vulkan renderers.
# The clone takes the tag, and the build stops when the tag is not at
# the pinned commit.
ARG WLROOTS_COMMIT=88a869855742281c98c22cab9641b317b8d065ef
RUN git clone --depth 1 --branch 0.19.3 https://gitlab.freedesktop.org/wlroots/wlroots.git /src/wlroots \
    && commit=$(git -C /src/wlroots rev-parse HEAD) \
    && if [ "$commit" != "$WLROOTS_COMMIT" ]; then \
         echo "wlroots 0.19.3 is at $commit, not at the pinned $WLROOTS_COMMIT" >&2; exit 1; \
       fi
RUN cd /src/wlroots && meson setup build --prefix=/usr/local \
    -Dbackends=drm,libinput -Drenderers=[] -Dallocators=gbm \
    -Dxwayland=disabled -Dexamples=false \
    && ninja -C build install

# labwc 0.9.8 (ext-image-copy-capture needs 0.9.0 or later), checked
# against its pinned commit as wlroots is.
ARG LABWC_COMMIT=42bee04163149e4209517d645435156ca8aff6de
RUN git clone --depth 1 --branch 0.9.8 https://github.com/labwc/labwc.git /src/labwc \
    && commit=$(git -C /src/labwc rev-parse HEAD) \
    && if [ "$commit" != "$LABWC_COMMIT" ]; then \
         echo "labwc 0.9.8 is at $commit, not at the pinned $LABWC_COMMIT" >&2; exit 1; \
       fi
RUN cd /src/labwc && env PKG_CONFIG_PATH=/usr/local/lib/pkgconfig:/usr/local/lib/aarch64-linux-gnu/pkgconfig:/usr/local/lib/x86_64-linux-gnu/pkgconfig \
    meson setup build --prefix=/usr/local -Dxwayland=disabled \
    && ninja -C build install

# cargo-auditable writes the list of the crates of pagis-screend into
# the executable, so Trivy identifies them in the image. The build takes
# the crate versions of Cargo.lock, which cargo-deny also checks.
FROM rust:1-trixie@sha256:a8a5f0a1e5fe7dfe1d352591e4a1c7dd2c08fd70475cae872cf3458ba0df0546 AS screend
ARG CARGO_AUDITABLE_VERSION=0.7.6
RUN cargo install --locked --version "$CARGO_AUDITABLE_VERSION" cargo-auditable
COPY screend /src/screend
RUN cd /src/screend && cargo auditable build --release --locked

# The scripting runtimes.
# uv and Node come from the vendors' own release tarballs, pinned by
# version and sha256, because Debian's node is version 20, which is
# past end of life. npm and pnpm come from their npm registry tarballs,
# pinned by version and by the sha512 of the registry's `integrity`
# field. The download tools stay in this stage; the final image keeps
# only the runtimes.
# tar that runs as root keeps the owner that an archive records. The uv
# and Node archives record uid 1001, which is `screen` in the final
# image, and the copy of /opt/node also gives its directory owners to
# /usr/local/bin, lib, include and share. So each extraction takes
# `--no-same-owner`, and root owns the runtimes and those directories
# (ADR-0013).
FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a AS runtimes
# buildx sets TARGETARCH; a plain `docker build` does not, so the steps
# below fall back to the build host's own architecture.
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl xz-utils \
    && rm -rf /var/lib/apt/lists/*

ARG UV_VERSION=0.12.9
ARG UV_SHA256_amd64=ec7a99cd05e0cd7f80243f135ce1361c76835cb0ee60055d14d20eba8eba1460
ARG UV_SHA256_arm64=c36fe17937ff6bd16dc42fc13854b5465999fcab2efe0af559381e945e3c6001
RUN set -eux; \
    case "${TARGETARCH:-$(dpkg --print-architecture)}" in \
      amd64) target=x86_64-unknown-linux-gnu; sha="$UV_SHA256_amd64" ;; \
      arm64) target=aarch64-unknown-linux-gnu; sha="$UV_SHA256_arm64" ;; \
      *) echo "unsupported architecture" >&2; exit 1 ;; \
    esac; \
    curl -fsSLo /tmp/uv.tar.gz \
      "https://github.com/astral-sh/uv/releases/download/${UV_VERSION}/uv-${target}.tar.gz"; \
    echo "$sha  /tmp/uv.tar.gz" | sha256sum -c -; \
    mkdir -p /opt/uv; \
    tar -xzf /tmp/uv.tar.gz -C /opt/uv --strip-components=1 --no-same-owner

ARG NODE_VERSION=24.20.0
ARG NODE_SHA256_amd64=2f2c0da162318f0de47665410c7c8c2ed3d36c8f3105de4bbc61176c70a7cbf2
ARG NODE_SHA256_arm64=5f4ddab610c1ab2016b3c227cebdbf6d9495161487e4739c7b90090595f465f7
RUN set -eux; \
    case "${TARGETARCH:-$(dpkg --print-architecture)}" in \
      amd64) arch=x64; sha="$NODE_SHA256_amd64" ;; \
      arm64) arch=arm64; sha="$NODE_SHA256_arm64" ;; \
      *) echo "unsupported architecture" >&2; exit 1 ;; \
    esac; \
    curl -fsSLo /tmp/node.tar.xz \
      "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${arch}.tar.xz"; \
    echo "$sha  /tmp/node.tar.xz" | sha256sum -c -; \
    mkdir -p /opt/node; \
    tar -xJf /tmp/node.tar.xz -C /opt/node --strip-components=1 --no-same-owner \
      --exclude CHANGELOG.md --exclude LICENSE --exclude README.md

# npm replaces the npm that the Node release bundles, so the image holds
# one npm only. The links in /opt/node/bin point into the package
# directory, so they reach the pinned npm.
ARG NPM_VERSION=11.20.0
ARG NPM_SHA512=745dc40c5c1b60df8de51522a7e6580ded0d79444ae4182a28e72f4f588db546214cc4e5d05c16841b97ad2ecab57c9dbaac93312e5a6906ab7a09c70c0c4d83
RUN set -eux; \
    curl -fsSLo /tmp/npm.tgz "https://registry.npmjs.org/npm/-/npm-${NPM_VERSION}.tgz"; \
    echo "$NPM_SHA512  /tmp/npm.tgz" | sha512sum -c -; \
    rm -rf /opt/node/lib/node_modules/npm; \
    mkdir /opt/node/lib/node_modules/npm; \
    tar -xzf /tmp/npm.tgz -C /opt/node/lib/node_modules/npm --strip-components=1 --no-same-owner

# pnpm goes into the Node tree, as npm does, so each uid runs the pinned
# version. The agent uid cannot read the home of root, so a copy that
# `corepack prepare` keeps there does not reach it.
ARG PNPM_VERSION=11.28.2
ARG PNPM_SHA512=755ce2fe3c05bfd7bb412b42aff3c5c55cd38c529c6eca810475497ae36746c571e4b6e1814e6e7e4b97d7ee864b8a0e44673c3b297a50e0ac28771c5ce15d27
RUN set -eux; \
    curl -fsSLo /tmp/pnpm.tgz "https://registry.npmjs.org/pnpm/-/pnpm-${PNPM_VERSION}.tgz"; \
    echo "$PNPM_SHA512  /tmp/pnpm.tgz" | sha512sum -c -; \
    mkdir /opt/node/lib/node_modules/pnpm; \
    tar -xzf /tmp/pnpm.tgz -C /opt/node/lib/node_modules/pnpm --strip-components=1 --no-same-owner; \
    ln -s ../lib/node_modules/pnpm/bin/pnpm.mjs /opt/node/bin/pnpm; \
    ln -s ../lib/node_modules/pnpm/bin/pnpx.mjs /opt/node/bin/pnpx

# The content blocker: the uBlock Origin Lite release the
# browser loads as an unpacked extension. It comes from the vendor's
# own release asset, pinned by tag and sha256, as uv and Node above do,
# and `third_party/ublock-origin-lite/` holds its license and names the
# release. The release is taken as it is published, with no fork and no
# rename; it is GPL-3.0-only and it runs as a separate program, so it
# does not reach the rest of Pagis. The unzip tool stays in this stage.
# The extension is one tree of JavaScript and rule data for every
# architecture, so this stage runs on the build host and not under
# emulation in the arm64 half of the two-architecture build.
FROM --platform=$BUILDPLATFORM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a AS blocker
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*
ARG UBOL_VERSION=2026.901.1442
ARG UBOL_SHA256=71a8c65573489e0702f6398d3d744fcbe2bbc48336a75781c90bec50acb00b7e
RUN set -eux; \
    mkdir -p /opt/pagis; \
    curl -fsSLo /tmp/ubol.zip \
      "https://github.com/uBlockOrigin/uBOL-home/releases/download/${UBOL_VERSION}/uBOLite_${UBOL_VERSION}.chromium.zip"; \
    echo "$UBOL_SHA256  /tmp/ubol.zip" | sha256sum -c -; \
    unzip -q /tmp/ubol.zip -d /opt/pagis/ublock-origin-lite

FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a
LABEL org.pagis.computer.version="0.20.0"
# GHCR reads the OCI source label to link the package to the
# repository, which is where a reader of the package page goes for the
# source and the licenses.
LABEL org.opencontainers.image.source="https://github.com/pagis-co/pagis"
LABEL org.opencontainers.image.description="The agent computer for Pagis: a headless Wayland desktop with Chromium, a terminal and the Pagis screen daemon."
# `ca-certificates` gives python3, Node, uv, pnpm and curl the
# public root store. Without it every HTTPS call from a script fails,
# and the agent falls back to the browser for what is one API call.
# The fonts cover the scripts and the emoji a page asks for:
# Noto for the world's scripts, Noto Color Emoji for the emoji, and
# croscore with crosextra for the metric substitutes of Arial, Times
# New Roman, Calibri and Cambria. Without them, twenty font files render
# every page in DejaVu and leave a font list that a bot scorer weighs.
RUN apt-get update && apt-get install -y --no-install-recommends \
    chromium foot sudo xkb-data \
    fonts-dejavu-core fonts-liberation fonts-noto-core \
    fonts-noto-color-emoji fonts-croscore fonts-crosextra-carlito \
    fonts-crosextra-caladea \
    wayland-utils libwayland-server0 libwayland-client0 libpixman-1-0 \
    libdrm2 libxkbcommon0 libcairo2 libpango-1.0-0 libpangocairo-1.0-0 \
    libglib2.0-0 libxml2 librsvg2-2 procps python3 ca-certificates \
    libinput10 libgbm1 libudev1 libseat1 libdisplay-info2 libliftoff0 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=compositor /usr/local /usr/local
RUN ldconfig /usr/local/lib || ldconfig
COPY --from=screend /src/screend/target/release/pagis-screend /usr/local/bin/pagis-screend

# The runtimes the agent scripts with: python3 above, uv for Python
# environments, and Node 24 LTS with npm and pnpm. The pnpm package is
# JavaScript that Node runs, so the standalone pnpm binary stays out of
# the image. The agent keeps each package's dependencies inside
# ~/software/<name> (`uv venv`, `pnpm install`).
COPY --from=runtimes /opt/uv/uv /opt/uv/uvx /usr/local/bin/
COPY --from=runtimes /opt/node /usr/local/

# The uid split (ADR-0013). `screen` runs the compositor, screend
# and Chromium; `agent` runs only the shell inside the terminal. The
# Wayland socket lives in a runtime directory only `screen` can read,
# so the agent's shell cannot build a Wayland client and inject input
# behind screend, and cannot read the browser profile. `foot` runs as
# `screen` — it has to draw — and drops to `agent` for the shell it
# hosts, which is the one privilege step sudo exists for here.
# Both homes live on the per-agent volume mounted at /data, so the
# browser profile survives a stop the way the agent's files do.
# sudo resets the environment, and the terminal's shell keeps the Exit
# Proxy entries of the container (ADR-0029), so the terminal sends its
# connections through the Exit Proxy as `computer_shell` does.
# `pagis-apt` keeps none of them, so apt connects directly.
RUN useradd -u 1000 -d /data/agent -s /bin/bash agent \
    && useradd -u 1001 -d /data/screen -s /bin/bash screen
RUN printf '%s\n' \
      'Defaults:screen env_keep += "HTTP_PROXY HTTPS_PROXY NO_PROXY http_proxy https_proxy no_proxy"' \
      'screen ALL=(agent) NOPASSWD: /bin/bash' \
      > /etc/sudoers.d/pagis-agent-shell \
    && chmod 0440 /etc/sudoers.d/pagis-agent-shell \
    && visudo -cf /etc/sudoers.d/pagis-agent-shell
# System packages. `pagis-apt` is the only command the agent uid
# may run as root, and the wrapper — not the caller — owns every option
# apt sees. A sudoers rule on apt-get
# itself gives root away through `-o`, `-c`, `--reinstall` or a local
# `.deb` path. sudo `env_reset` stays on, so no APT_CONFIG crosses.
COPY pagis-apt /usr/local/sbin/pagis-apt
RUN chown root:root /usr/local/sbin/pagis-apt \
    && chmod 0755 /usr/local/sbin/pagis-apt \
    && printf 'agent ALL=(root) NOPASSWD: /usr/local/sbin/pagis-apt\n' \
       > /etc/sudoers.d/pagis-apt \
    && chmod 0440 /etc/sudoers.d/pagis-apt
# Image-owned config lives outside the volume: the per-agent named
# volume mounts at /data and must carry user data only, so image
# updates reach existing agents.
COPY labwc /etc/pagis/labwc
# The first-party Skills (ADR-0017). The daemon lists and loads
# the same files, and each granted Plugin's `skills/` mounts read-only
# under /opt/plugins, so every Skill the agent reads has one shape.
COPY skills /opt/pagis/skills
RUN mkdir -p /opt/plugins
# The fingerprint page. It reads the timezone, the language, the
# font count, the WebGL renderer and navigator.webdriver in the real
# browser and writes them into the document title, which screend
# reports on GET /windows. The gate test opens it to hold each of those
# values, because each one is one image detail or launch flag away from
# a page that sites serve to bots.
COPY fingerprint /opt/pagis/fingerprint
# Chromium's own password manager is off (ADR-0013): the daemon keeps
# secrets, and anything the browser saved would sit on a volume the
# agent can otherwise reach.
# `PromotionsEnabled` is false, so Chromium shows no promotion of its
# own features. Without it, a "Sign in to Chromium?" promo opens on the
# avatar button when the browser starts and closes 20 s later. That
# change is larger than the settle tolerance (ADR-0014), and it is noise
# in the screenshots the agent reads. `BrowserSignin` does not stop the
# promo.
COPY chromium-policies /etc/chromium/policies/managed
# The policy file of the Exit Proxy (ADR-0029). screend runs as `screen`
# and rewrites its contents at each mode: `WebRtcIPHandling` in Home
# mode, and `{}` in Direct mode. The directory stays root's, so screend
# changes this one file and adds no other policy, and the agent uid
# reads the file and writes nothing there.
RUN printf '{}\n' > /etc/chromium/policies/managed/pagis-exit.json \
    && chown screen:screen /etc/chromium/policies/managed/pagis-exit.json \
    && chmod 0644 /etc/chromium/policies/managed/pagis-exit.json
# The content blocker, fetched in the `blocker` stage above.
# browser.sh gives the tree to Chromium as an unpacked extension. It
# belongs to `screen`, the uid that runs the browser: Chromium indexes
# the extension's static filter rules into `_metadata/` inside the
# extension directory, and a directory it cannot write refuses the
# whole extension. The agent uid cannot write there, so the agent's
# shell cannot change what the browser blocks.
COPY --from=blocker --chown=screen:screen \
    /opt/pagis/ublock-origin-lite /opt/pagis/ublock-origin-lite
COPY entrypoint.sh /entrypoint.sh
# The browser launcher: screend runs it with the DevTools pipe on its
# standard streams, and starts it again when the browser exits.
COPY browser.sh /usr/local/bin/pagis-browser
RUN chmod +x /entrypoint.sh /usr/local/bin/pagis-browser
# The clock and the locale. The daemon gives each container the
# Workspace timezone in TZ and en_US.UTF-8 in LANG at create time; this
# image holds the zone data and the one generated locale those names
# need. A container whose clock and language disagree with its egress
# IP looks automated to the sites the agent visits.
RUN apt-get update && apt-get install -y --no-install-recommends locales tzdata \
    && sed -i 's/^# *en_US.UTF-8 UTF-8/en_US.UTF-8 UTF-8/' /etc/locale.gen \
    && locale-gen en_US.UTF-8 \
    && rm -rf /var/lib/apt/lists/*
ENV XDG_RUNTIME_DIR=/run/pagis-xdg \
    WLR_BACKENDS=headless \
    WLR_RENDERER=pixman \
    WLR_LIBINPUT_NO_DEVICES=1 \
    WAYLAND_DISPLAY=wayland-0
# The screend control endpoint: /healthz, /frame.png, /input,
# /offer, /windows, /holder, /browser/*, /exit — and the WebRTC media UDP
# port. The Exit Proxy of screend listens on 127.0.0.1:3128 alone, so it
# publishes no port.
# Reaching the port is not holding it: every endpoint but
# /healthz needs the bearer token that entrypoint.sh copies out of the
# mount at /run/pagis-token/screend.
EXPOSE 7900
EXPOSE 7901/udp
ENTRYPOINT ["/entrypoint.sh"]
