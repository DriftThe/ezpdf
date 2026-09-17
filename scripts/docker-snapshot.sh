#!/usr/bin/env bash
# Split an already-built pyserver image into parts and upload to a Release.
#
#   bash scripts/docker-snapshot.sh <cpu|gpu> <tag> [release]
#     arg2 names the files (build-source tag); arg3 is the target Release (defaults to arg2).
# Images ship separately from the app installers as pyserver-<app tag>. Parts because a Release
# asset is capped at 2 GiB while the tar is ~2.1GB (CPU) / ~4.5GB (GPU); downloaders merge
# (cat / copy /b) then docker load. Needs the image, a logged-in gh, and GNU split/sha256sum.
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
