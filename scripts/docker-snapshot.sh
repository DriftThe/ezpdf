#!/usr/bin/env bash
# 把一个已构建好的解析服务镜像打成分片快照并上传到 Release。
#
#   bash scripts/docker-snapshot.sh cpu v0.1.1
#   bash scripts/docker-snapshot.sh gpu v0.1.1
#
# 前置：镜像已存在（ezpdf-pyserver:cpu / :gpu，见 pyserver/Dockerfile），
#       本机已登录 gh（CI 里用 GITHUB_TOKEN），依赖 GNU coreutils 的 split/sha256sum（Linux）。
#
# 为什么要切分：GitHub Release 单个附件上限 2GiB，而实测 tar 为 CPU 2.1GB / GPU 4.5GB。
# 客户端下载后按 README「用 Docker 部署解析服务」里的说明合并（cat 或 copy /b）再 docker load。
set -euo pipefail

variant="${1:?usage: docker-snapshot.sh <cpu|gpu> <tag>}"
tag="${2:?usage: docker-snapshot.sh <cpu|gpu> <tag>}"

tar="ezpdf-pyserver-${tag}-${variant}.tar"
sha="ezpdf-pyserver-${tag}-${variant}.tar.sha256"

# 同时打上版本 tag：载入后既有 ezpdf-pyserver:cpu 这个稳定名，也能看出快照属于哪一版
docker save -o "$tar" "ezpdf-pyserver:${variant}" "ezpdf-pyserver:${tag}-${variant}"
sha256sum "$tar" > "$sha"

split -b 1900M -d --suffix-length=2 "$tar" "${tar}.part-"
rm -f "$tar"

ls -l "${tar}".part-* "$sha" | awk '{printf "%12d  %s\n", $5, $9}'
gh release upload "$tag" "${tar}".part-* "$sha" --clobber

# 分片上传完就删掉：GPU 快照前还要腾一次盘（tar + 镜像同时在会很占地方）
rm -f "${tar}".part-*
