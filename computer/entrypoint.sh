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

# The control token. The daemon bind-mounts the token file
# read-only at /run/pagis-token/screend. Its owner is the host uid that
# wrote it, a uid this image does not know, so the mode of the mounted
# file protects nothing here. The copy below belongs to `screen`, the
# uid that runs screend, and 0700 on the mount point puts the source
# out of reach of both other uids afterwards — also in the case where
# the host uid is 1000, which is `agent` in this image. The token stays
# out of the environment: every `docker exec`, the agent's own shell
# among them, inherits it. A container that boots with no mounted token
# still boots; screend then refuses every control request.
mkdir -p /run/pagis
chown root:root /run/pagis
chmod 755 /run/pagis
if [ -f /run/pagis-token/screend ]; then
    cp /run/pagis-token/screend /run/pagis/screend-token
    chown screen:screen /run/pagis/screend-token
    chmod 400 /run/pagis/screend-token
    chmod 700 /run/pagis-token
fi

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
