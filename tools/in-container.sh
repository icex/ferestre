#!/bin/sh
# Run a command inside the Proton SDK container the Xodus build uses, with
# the same source/build mounts. Needs the docker group: pipe through
# `newgrp docker` if the shell doesn't have it yet.
#
# Three mounts, because the three trees no longer share a parent: this
# repository (the tests compile sources out of it), the Proton and client
# sources (third_party/ by default), and the build tree.
. "$(dirname "$0")/../scripts/xodus-env.sh"
IMAGE=registry.gitlab.steamos.cloud/proton/steamrt4/sdk/x86_64:4.0.20260331.220802-0
exec docker run --rm \
    -v "$XODUS_REPO_DIR:$XODUS_REPO_DIR" \
    -v "$XODUS_SRC_DIR:$XODUS_SRC_DIR" \
    -v "$XODUS_BUILD_DIR:$XODUS_BUILD_DIR" \
    -w "${WORKDIR:-$XODUS_BUILD_DIR}" \
    --user "$(id -u):$(id -g)" -e HOME=/tmp "$IMAGE" "$@"
