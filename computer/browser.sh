#!/bin/sh
# The browser launcher. screend starts it, and starts it again when the
# browser exits (see computer/screend/src/browser.rs).
#
# screend gives this script the two ends of the DevTools pipe as its
# standard input and output. The `exec 3<&0 4>&1` line below moves them
# to the descriptors 3 and 4, which --remote-debugging-pipe reads
# commands from and writes answers to. It gives Chromium /dev/null and
# stderr as its own standard streams, so nothing that Chromium prints
# enters the pipe.
# The pipe has no name in the file system and no port: only screend and
# Chromium hold it, and both run as `screen`. The daemon writes a Vault
# fill through it (ADR-0013). Do not add --remote-debugging-port: a port
# is reachable from the Agent's shell.
#
# Chromium marks a browser that has a DevTools pipe as controlled by
# automation, and every page then reads navigator.webdriver as true,
# which is what a bot scorer looks for.
# --disable-blink-features=AutomationControlled keeps the value false.
# Chromium shows the "unsupported command-line flag" bar for that flag in
# every screenshot the agent reads, and --test-type stops the bar. A page
# sees neither flag.
#
# Chromium keeps its own sandbox on. The daemon starts the
# container under a seccomp profile that permits the user-namespace
# calls (crates/pagis-computer/seccomp/chromium.json), which is what
# the renderer sandbox needs. Do not add --no-sandbox: it turns
# the sandbox off in the one process that runs untrusted code, and it
# puts the "unsupported command-line flag" bar in every screenshot the
# agent reads.
#
# The GL flags give the browser a WebGL renderer. The computer
# has no GPU and the compositor draws with pixman, so the browser
# finds no native GL and opens no WebGL context: maps and charts stay
# blank, and a scorer reads the missing renderer as a bot. ANGLE on
# SwiftShader draws WebGL on the CPU and reports an ANGLE renderer
# string. The Debian chromium package carries SwiftShader
# (/usr/lib/chromium/libvk_swiftshader.so), so no extra package is
# necessary. Chromium refuses a software WebGL context without
# --enable-unsafe-swiftshader.
#
# uBlock Origin Lite loads unpacked from the image. Ads,
# trackers and consent scripts each cost the agent a screenshot and
# the tokens to read it, and an ad frame can take the focus off the
# page. --disable-extensions-except keeps the blocker the only
# extension the browser accepts. See third_party/ublock-origin-lite.
UBOL=/opt/pagis/ublock-origin-lite
exec 3<&0 4>&1 </dev/null >&2
exec chromium --ozone-platform=wayland --start-maximized \
    --use-gl=angle --use-angle=swiftshader --enable-unsafe-swiftshader \
    --disable-features=WaylandFractionalScaleV1 \
    --hide-crash-restore-bubble \
    --lang=en-US \
    --load-extension="$UBOL" --disable-extensions-except="$UBOL" \
    --remote-debugging-pipe \
    --disable-blink-features=AutomationControlled --test-type \
    --no-first-run --disable-background-networking about:blank
