#!/bin/sh
# The agent computer entrypoint. It runs as root only to
# lay out the two homes on the per-agent volume and the runtime
# directory, then hands the session to the unprivileged `screen` user;
# labwc is the session supervisor and runs the autostart programs
# (screend, foot, Chromium).
set -e

# The volume mounts empty on first wake. `/data` is its mount point,
# not a home: Docker makes it root-owned and 0755, so both uids
# traverse it to reach their homes underneath. `screen` owns the
# browser profile and the Wayland socket; `agent` owns nothing the
# session needs, and cannot read either.
mkdir -p /data/agent /data/screen
chown agent:agent /data/agent
chmod 700 /data/agent
chown screen:screen /data/screen
chmod 700 /data/screen

# The Wayland socket lives here. 0700 under `screen` is what puts it
# out of the agent shell's reach (ADR-0013).
mkdir -p "$XDG_RUNTIME_DIR"
chown screen:screen "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

# The daemon uploads /etc/pagis/screend-token before each start.
# The image owns /etc/pagis as root:screen with mode 0750, and the
# token is root:screen with mode 0440. Only root and screen read it.
# The token stays outside /run, so a tmpfs there does not hide it.
# It stays out of the environment that every docker exec inherits.

# Chromium records <hostname>-<pid> in SingletonLock. The profile lives
# in the per-agent volume, but each container gets a new hostname, so a
# lock from the previous container reads as "another computer" and
# Chromium refuses to start. One container runs one Chromium, so any
# lock found at session start is stale.
rm -f /data/screen/.config/chromium/SingletonLock \
      /data/screen/.config/chromium/SingletonCookie \
      /data/screen/.config/chromium/SingletonSocket

exec setpriv --reuid=screen --regid=screen --init-groups --inh-caps=-all \
    env HOME=/data/screen USER=screen LOGNAME=screen \
    labwc -C /etc/pagis/labwc
