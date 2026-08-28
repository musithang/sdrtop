# Sourced by the packaging scripts: pick a container runtime.
#
# podman locally, docker on a GitHub runner. `CONTAINER` overrides both, for a
# machine where the wrong one is first on PATH.
if [ -z "${CONTAINER:-}" ]; then
    if command -v podman >/dev/null 2>&1; then
        CONTAINER=podman
    elif command -v docker >/dev/null 2>&1; then
        CONTAINER=docker
    else
        echo "no container runtime: install podman or docker" >&2
        exit 2
    fi
fi

# Rootless podman maps the container's root to the invoking user, so files land
# owned by whoever ran the build. Docker does not: without this, every artefact
# comes back owned by root and the next local build cannot overwrite it.
CONTAINER_USER=""
if [ "$CONTAINER" = docker ]; then
    CONTAINER_USER="--user $(id -u):$(id -g)"
fi
