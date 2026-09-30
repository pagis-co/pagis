#!/bin/sh
# Import the Developer ID Application identity of CSC_LINK (a base64
# `.p12`) and CSC_KEY_PASSWORD into a keychain of the job, and put that
# keychain on the search list, where `codesign` and `security
# find-identity` find it. The keychain lives in the temporary directory of
# the runner, which the runner removes with the job.
#
# The later steps of the job get the fingerprint of the identity as
# CSC_NAME. The release signs the server and the client with it, and
# electron-builder then uses this keychain instead of an import of its own.
set -eu
: "${CSC_LINK:?CSC_LINK is required}"
: "${CSC_KEY_PASSWORD:?CSC_KEY_PASSWORD is required}"
: "${RUNNER_TEMP:?RUNNER_TEMP is required}"

keychain="$RUNNER_TEMP/signing.keychain-db"
certificate="$RUNNER_TEMP/signing.p12"
password=$(openssl rand -hex 24)
trap 'rm -f "$certificate"' EXIT

printf '%s' "$CSC_LINK" | base64 --decode > "$certificate"
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$certificate" -k "$keychain" -P "$CSC_KEY_PASSWORD" -f pkcs12 -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -k "$password" "$keychain" >/dev/null
# shellcheck disable=SC2046 # each keychain of the list is one word
security list-keychains -d user -s "$keychain" $(security list-keychains -d user | tr -d '"')
fingerprint=$(security find-identity -v -p codesigning "$keychain" \
  | awk '/"Developer ID Application:/ { print $2; exit }')
[ -n "$fingerprint" ] || {
  echo 'CSC_LINK holds no Developer ID Application identity' >&2
  exit 1
}
if [ -n "${GITHUB_ENV:-}" ]; then
  echo "CSC_NAME=$fingerprint" >> "$GITHUB_ENV"
fi
