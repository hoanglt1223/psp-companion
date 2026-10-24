#!/usr/bin/env sh
# Builds psp/EBOOT.PBP inside the pspdev/pspdev Docker image.
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
image="${PSPDEV_IMAGE:-pspdev/pspdev:latest}"
MSYS_NO_PATHCONV=1 docker run --rm -v "$root/psp:/src" -w /src "$image" make "$@"
