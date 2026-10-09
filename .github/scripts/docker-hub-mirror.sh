#!/usr/bin/env bash
# Make the Docker daemon of a GitHub-hosted runner pull each Docker Hub
# image through mirror.gcr.io, the public Docker Hub mirror of Google.
# Docker Hub refuses the anonymous pulls of a shared runner address with
# `429 Too Many Requests`. The daemon pulls from Docker Hub when the
# mirror does not hold the image.
#
# The restart stops each running container, so a job runs this script
# before it starts a buildx builder or a container.
set -euo pipefail

config=/etc/docker/daemon.json
current=$(sudo cat "$config" 2>/dev/null || echo '{}')
echo "$current" \
  | jq '. + {"registry-mirrors": ["https://mirror.gcr.io"]}' \
  | sudo tee "$config" >/dev/null
sudo systemctl restart docker
docker info --format '{{.RegistryConfig.Mirrors}}'
