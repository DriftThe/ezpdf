#!/usr/bin/env bash
# Split an already-built pyserver image into part files and upload them to a Release.
#
#   bash scripts/docker-snapshot.sh cpu v0.1.2 pyserver-v0.1.2
#   bash scripts/docker-snapshot.sh gpu v0.1.2 pyserver-v0.1.2
#     arg2 = build-source tag (names the files), arg3 = target Release
#     (images ship separately from installers: pyserver-<app tag>; defaults to arg2)
#
# Requires the image (ezpdf-pyserver:cpu / :gpu, see pyserver/Dockerfile), a logged-in gh
# (GITHUB_TOKEN in CI), and GNU coreutils split/sha256sum (Linux).
#
# Parts are needed because a GitHub Release asset is capped at 2 GiB while the tar is
# ~2.1GB (CPU) / ~4.5GB (GPU); downloaders merge them (cat or copy /b) then docker load.
set -euo pipefail

variant="${1:?usage: docker-snapshot.sh <cpu|gpu> <tag> [release]}"
tag="${2:?usage: docker-snapshot.sh <cpu|gpu> <tag> [release]}"
release="${3:-$tag}"

tar="ezpdf-pyserver-${tag}-${variant}.tar"
sha="ezpdf-pyserver-${tag}-${variant}.tar.sha256"

# Also tag with the version, so a loaded image keeps the stable :cpu name and shows its snapshot
docker save -o "$tar" "ezpdf-pyserver:${variant}" "ezpdf-pyserver:${tag}-${variant}"
sha256sum "$tar" > "$sha"

split -b 1900M -d --suffix-length=2 "$tar" "${tar}.part-"
rm -f "$tar"

ls -l "${tar}".part-* "$sha" | awk '{printf "%12d  %s\n", $5, $9}'
gh release upload "$release" "${tar}".part-* "$sha" --clobber

# Delete the parts after upload to free disk before a GPU snapshot (tar + image is large)
rm -f "${tar}".part-*
