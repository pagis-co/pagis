#!/bin/sh
# Write the App Store Connect API key of APPLE_API_KEY_P8 (the text of its
# `.p8` file) to the temporary directory of the runner, and name the file
# in APPLE_API_KEY for the later steps of the job. notarytool,
# electron-builder, xcodebuild and altool read the key from that file.
set -eu
: "${APPLE_API_KEY_P8:?APPLE_API_KEY_P8 is required}"
: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${GITHUB_ENV:?GITHUB_ENV is required}"

key="$RUNNER_TEMP/notary-api-key.p8"
umask 077
printf '%s\n' "$APPLE_API_KEY_P8" > "$key"
echo "APPLE_API_KEY=$key" >> "$GITHUB_ENV"
