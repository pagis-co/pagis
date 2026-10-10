#!/usr/bin/env bash
# Gives a git worktree its own Cargo target directory, seeded from the
# main checkout's target/.
#
#   scripts/seed-worktree-target.sh
#
# Run it in a new worktree before its first cargo command, while no build
# runs in the main checkout. The copy is a copy-on-write clone (APFS on
# macOS, reflink on Btrfs and XFS), so the worktree shares the disk blocks
# of the compiled third-party crates until Cargo writes over them. The
# script then removes the fingerprints of the workspace members, so Cargo
# builds each member again from the sources of this worktree.
#
# The main checkout, and a worktree in which Cargo has built, keep their
# target directory as it is.
set -euo pipefail

worktree="$(git rev-parse --show-toplevel)"
main="$(git worktree list --porcelain | sed -n '1s/^worktree //p')"

# Cargo keeps a fingerprint in target/<profile>/.fingerprint, or in
# target/<triple>/<profile>/.fingerprint for a cross build. Its directory
# name is the package name, a dash and the unit hash. Other tools, such
# as nextest, can make target/ before the first build.
fingerprints() {
  compgen -G "$1/target/*/.fingerprint" || compgen -G "$1/target/*/*/.fingerprint"
}

if [[ "$worktree" == "$main" ]]; then
  echo "This is the main checkout. Its target/ stays as it is."
  exit 0
fi
if fingerprints "$worktree" >/dev/null; then
  echo "Cargo has built in this worktree. Its target/ stays as it is."
  exit 0
fi
if [[ ! -d "$main/target" ]]; then
  echo "The main checkout has no target/ directory. Cargo builds this worktree from the start."
  exit 0
fi

mkdir -p "$worktree/target"
case "$(uname -s)" in
  Darwin) cp -c -R -p "$main/target/" "$worktree/target" ;;
  *) cp -a --reflink=auto "$main/target/." "$worktree/target" ;;
esac

cargo metadata --no-deps --format-version 1 --manifest-path "$worktree/Cargo.toml" |
  jq -r '.packages[].name' |
  while read -r member; do
    rm -rf "$worktree"/target/*/.fingerprint/"$member"-* \
      "$worktree"/target/*/*/.fingerprint/"$member"-*
  done

echo "Seeded $worktree/target from $main/target."
