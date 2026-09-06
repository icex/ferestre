#!/bin/sh
# Run a command inside the Proton SDK container the Xodus build uses, with
# the same source/build mounts. Needs the docker group: pipe through
# `newgrp docker` if the shell doesn't have it yet.
IMAGE=registry.gitlab.steamos.cloud/proton/steamrt4/sdk/x86_64:4.0.20260331.220802-0
exec docker run --rm -v ${XODUS_SRC_DIR:-$HOME/src}:${XODUS_SRC_DIR:-$HOME/src} -w "${WORKDIR:-${XODUS_SRC_DIR:-$HOME/src}/xodus-build}" \
    --user "$(id -u):$(id -g)" -e HOME=/tmp "$IMAGE" "$@"
