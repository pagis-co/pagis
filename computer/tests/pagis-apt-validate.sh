#!/usr/bin/env bash
# The validator tests for computer/pagis-apt. They source the
# wrapper, which then only defines its functions, so no test touches
# apt or needs Docker.
#
#   bash computer/tests/pagis-apt-validate.sh
#
# `cargo xtask full` runs this file as the `pagis-apt` step.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../pagis-apt
source "$here/../pagis-apt"

failures=0

reject() {
  local name="$1"
  if valid_package_name "$name"; then
    printf 'FAIL: accepted %q, expected a refusal\n' "$name" >&2
    failures=$((failures + 1))
  else
    printf 'ok: refused %q\n' "$name"
  fi
}

accept() {
  local name="$1"
  if valid_package_name "$name"; then
    printf 'ok: accepted %q\n' "$name"
  else
    printf 'FAIL: refused %q, expected an acceptance\n' "$name" >&2
    failures=$((failures + 1))
  fi
}

# Option injection: the vectors that give root away when apt sees them.
reject '-o'
reject '-o DPkg::Post-Invoke::=/bin/true'
reject '--reinstall'
reject '-c'
reject '--allow-unauthenticated'

# Local package files: apt installs these and runs their maintainer
# scripts as root. `--` does not stop them; the name pattern does.
reject './x.deb'
reject '/tmp/x.deb'
reject '../x.deb'
reject 'dir/x.deb'

# Shell and field splitting.
reject 'jq curl'
reject 'jq;curl'
reject 'jq$(id)'
reject 'jq
curl'
reject ''
reject ' jq'
reject 'JQ'
reject '.hidden'
reject '+plus'

# Real package names.
accept 'jq'
accept 'libfoo-dev'
accept 'g++'
accept 'ripgrep'
accept 'python3.13'
accept 'fd-find'
accept '7zip'

# The grammar: only `install`, and it needs at least one package.
usage_rejects=(
  ''
  'remove'
  'update'
  'install'
  '-install'
)
for line in "${usage_rejects[@]}"; do
  # shellcheck disable=SC2086
  if parse_command $line >/dev/null 2>&1; then
    printf 'FAIL: accepted the command %q, expected a refusal\n' "$line" >&2
    failures=$((failures + 1))
  else
    printf 'ok: refused the command %q\n' "$line"
  fi
done

if parse_command install jq libfoo-dev >/dev/null 2>&1; then
  printf 'ok: accepted the command "install jq libfoo-dev"\n'
else
  printf 'FAIL: refused the command "install jq libfoo-dev"\n' >&2
  failures=$((failures + 1))
fi

if [ "$failures" -ne 0 ]; then
  printf '\n%d test(s) failed\n' "$failures" >&2
  exit 1
fi
printf '\nall pagis-apt validator tests passed\n'
