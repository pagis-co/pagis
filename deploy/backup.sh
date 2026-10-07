#!/bin/sh
# Back up one Pagis server.
#
#   deploy/backup.sh /var/backups/pagis/nightly
#
# The three parts of an installation, captured together:
#
#   1. The state directory and the database, as one archive. The daemon
#      is stopped for it, because a file copied while the daemon writes
#      it is not a backup, and `pagis backup` refuses to run beside a
#      live daemon. The archive leaves out the logs and
#      `computer-tokens/`: a Computer token is live while its Computer
#      runs, and the Computers run on while the daemon is stopped.
#   2. The Computer volumes of this installation's Workspaces,
#      `pagis-volume-<workspace_id>-<agent_id>`, which belong to the
#      Docker host rather than to the daemon. The Docker host can hold
#      the volumes of other installations too; they are not taken.
#   3. The Installation Key, which is NOT copied here: it seals
#      `secrets.enc` and an archive holding both is a lock beside its
#      key. Keep `deploy/secrets/pagis-secrets-key` wherever the
#      deployment keeps its other secrets. Without it a restored server
#      opens no stored secret.
#
# https://docs.pagis.co/server/backup holds the procedure and the restore.
set -eu

if [ $# -ne 1 ]; then
	echo "usage: $0 <directory>" >&2
	exit 2
fi
destination=$1
cd "$(dirname "$0")"

if [ -e "$destination" ] && [ -n "$(ls -A "$destination" 2>/dev/null)" ]; then
	echo "$destination is not empty" >&2
	exit 1
fi
# Only the owner can go into the destination. The containers below write
# their files under their own umask, and this mode keeps other users of
# the host out of all of them.
umask 077
mkdir -p "$destination/volumes"
chmod 700 "$destination"
destination=$(cd "$destination" && pwd)

echo "== stopping the daemon"
docker compose stop pagis

# `restart: unless-stopped` brings the daemon back whatever happens
# next, so the shutdown of a failed backup is not a stopped server.
trap 'docker compose start pagis' EXIT

echo "== the state directory and the database"
docker compose run --rm --no-deps -v "$destination:/backup" \
	pagis backup /backup/installation

echo "== the Computer volumes"
# The Workspaces in this installation's database. Every Computer volume
# carries the label of its Workspace, so a volume of another
# installation on this Docker host has a label that is not in the list.
workspaces=$(docker compose exec -T db psql -U pagis -d pagis -Atc 'SELECT id FROM workspaces')
for workspace in $workspaces; do
	for volume in $(docker volume ls --quiet --filter "label=co.pagis.workspace=$workspace"); do
		echo "   $volume"
		docker run --rm \
			-v "$volume:/volume:ro" \
			-v "$destination/volumes:/backup" \
			alpine:3 tar -czf "/backup/$volume.tar.gz" -C /volume .
	done
done

echo "backed up into $destination"
