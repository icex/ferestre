#!/bin/bash
# Run a command inside the Proton SDK container the Xodus build uses, with
# the same source/build mounts. Needs the docker group: pipe through
# `newgrp docker` if the shell doesn't have it yet.
#
# Three mounts, because the three trees no longer share a parent: this
# repository (the tests compile sources out of it), the Proton and client
# sources (third_party/ by default), and the build tree.
#
# When Docker runs on another machine (a `docker context` or DOCKER_HOST that
# is not a local socket), host paths cannot be mounted. Then only a single
# compiler call is supported, which is what the tests/run-*.sh suites use to
# build their test binary: the sources and -I directories it names are sent
# into the container as a tar stream and the output file comes back on stdout.
# Anything else, such as `make` in the build tree, is refused with a message;
# run that on the host or against a local Docker. IN_CONTAINER_REMOTE=1 or =0
# overrides the detection.
. "$(dirname "$0")/../scripts/xodus-env.sh"
IMAGE=registry.gitlab.steamos.cloud/proton/steamrt4/sdk/x86_64:4.0.20260331.220802-0

remote=${IN_CONTAINER_REMOTE:-}
if [ -z "$remote" ]; then
    host=${DOCKER_HOST:-$(docker context inspect --format '{{.Endpoints.docker.Host}}' 2>/dev/null)}
    case "$host" in ''|unix://*) remote=0 ;; *) remote=1 ;; esac
fi

if [ "$remote" != 1 ]; then
    exec docker run --rm \
        -v "$XODUS_REPO_DIR:$XODUS_REPO_DIR" \
        -v "$XODUS_SRC_DIR:$XODUS_SRC_DIR" \
        -v "$XODUS_BUILD_DIR:$XODUS_BUILD_DIR" \
        -w "${WORKDIR:-$XODUS_BUILD_DIR}" \
        --user "$(id -u):$(id -g)" -e HOME=/tmp "$IMAGE" "$@"
fi

case "${1:-}" in
    *-gcc|*-g++|*-clang|gcc|cc|clang) ;;
    *) echo "in-container.sh: Docker is remote, so only a compiler call can run there, not: $*" >&2
       echo "in-container.sh: run it on the host, or with a local Docker (IN_CONTAINER_REMOTE=0)" >&2
       exit 2 ;;
esac
compiler=$1; shift
args=(); files=(); out=
while [ $# -gt 0 ]; do
    case "$1" in
        -o) out=$2; args+=(-o /w/out.bin); shift 2 ;;
        -I) files+=("$2"); args+=("-I/w$(realpath -m "$2")"); shift 2 ;;
        -I*) dir=$(realpath -m "${1#-I}"); files+=("$dir"); args+=("-I/w$dir"); shift ;;
        -*) args+=("$1"); shift ;;
        *) file=$(realpath -m "$1"); files+=("$file"); args+=("/w$file"); shift ;;
    esac
done
[ -n "$out" ] || { echo "in-container.sh: a remote compile needs -o" >&2; exit 2; }
quoted=$(printf ' %q' "$compiler" "${args[@]}")
tar -cf - -P --transform 's,^/,,' "${files[@]}" \
    | docker run -i --rm "$IMAGE" sh -c "mkdir -p /w && cd /w && tar -xf - && $quoted >&2 && cat /w/out.bin" \
    > "$out.part" && [ -s "$out.part" ] && mv "$out.part" "$out" || { rm -f "$out.part"; exit 1; }
