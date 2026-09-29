#!/bin/sh
# Restore one Pagis server onto a fresh host.
#
#   deploy/restore.sh /var/backups/pagis/nightly
#
# A fresh host: this machine holds `compose.yaml`, `.env` and
# `secrets/pagis-secrets-key`, and no Pagis volumes at all. The restore
# refuses a state directory that already holds an installation, because
# two installations in one directory are neither.
#
# The Installation Key is not in the archive. Put it back first, from
# wherever the deployment keeps its secrets; without it the restored
# server opens no stored secret.
#
# The server that opens restored data must be the release the archive
# names or newer: the release marker is one-way (ADR-0025).
# https://docs.pagis.co/server/backup holds the procedure.
set -eu

if [ $# -ne 1 ]; then
	echo "usage: $0 <directory>" >&2
	exit 2
fi
archive=$(cd "$1" && pwd)
cd "$(dirname "$0")"

if [ ! -f secrets/pagis-secrets-key ]; then
	echo "secrets/pagis-secrets-key is missing; put the Installation Key back first" >&2
	exit 1
fi

# A volume of the archive that the host already holds belongs to an
# installation that is here now. Extracting over it mixes two Computer
# homes, so the restore stops before it changes anything.
for tarball in "$archive"/volumes/*.tar.gz; do
	[ -e "$tarball" ] || break
	volume=$(basename "$tarball" .tar.gz)
	if docker volume inspect "$volume" >/dev/null 2>&1; then
		echo "the volume $volume already exists on this host; restore onto a host without it" >&2
		exit 1
	fi
done

echo "== the empty database"
docker compose up -d db
until docker compose exec -T db pg_isready -U pagis -d pagis >/dev/null 2>&1; do
	sleep 1
done

echo "== the state directory and the database"
docker compose run --rm -v "$archive:/backup:ro" \
	pagis restore /backup/installation

echo "== the Computer volumes"
for tarball in "$archive"/volumes/*.tar.gz; do
	[ -e "$tarball" ] || break
	volume=$(basename "$tarball" .tar.gz)
	echo "   $volume"
	docker volume create "$volume" >/dev/null
	docker run --rm \
		-v "$volume:/volume" \
		-v "$archive/volumes:/backup:ro" \
		alpine:3 tar -xzf "/backup/$(basename "$tarball")" -C /volume
done

echo "== starting the server"
docker compose up -d
