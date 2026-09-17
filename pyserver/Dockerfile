# ezpdf parse service image (CPU / GPU, switched by the VARIANT build arg)
#
#   docker build -t ezpdf-pyserver:cpu --build-arg VARIANT=cpu pyserver
#   docker build -t ezpdf-pyserver:gpu --build-arg VARIANT=gpu pyserver
#
# Models (models/, ~1.9GB) are COPYed straight in → a self-contained snapshot, inference-ready offline.
# CUDA deps come from pip's nvidia-* wheels (host only needs the driver + nvidia-container-toolkit),
# so the GPU variant is also based on python:3.12-slim, no nvidia/cuda base image.
# The base image is swappable (Docker Hub is unreachable in some regions):
#   --build-arg BASE_IMAGE=docker.m.daocloud.io/library/python:3.12-slim
ARG BASE_IMAGE=python:3.12-slim
FROM ${BASE_IMAGE}

ARG VARIANT=cpu
# Mirrors for slow/blocked builds (the same ones the client's "use mirror" switch uses):
#   --build-arg APT_MIRROR=mirrors.tuna.tsinghua.edu.cn
#   --build-arg PIP_INDEX=https://pypi.tuna.tsinghua.edu.cn/simple
#   --build-arg TORCH_INDEX=https://mirror.sjtu.edu.cn/pytorch-wheels/cpu   # GPU variant: .../cu132
ARG APT_MIRROR=""
ARG PIP_INDEX=""
ARG TORCH_INDEX=""

ENV PYTHONUNBUFFERED=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    EZPDF_HOST=0.0.0.0 \
    EZPDF_PORT=9055 \
    EZPDF_MODELS_DIR=/app/models \
    EZPDF_TOKEN_FILE=/data/token.txt \
    EZPDF_MAX_BATCH_PAGES=32

# opencv-contrib-python needs libgl/libglib (slim images lack them); kept in sync with the local .venv —
# no headless swap, since the image is a faithful package of a verified env and packages are not trimmed for size
# build-essential: on the CUDA path the pipeline JIT-compiles a triton kernel at runtime
#   ("Failed to find C compiler" on GPU); triton's JIT needs gcc/g++ and libc6 headers, usually present
#   on bare-metal Linux but required explicitly in a container
# Optional apt mirror (deb.debian.org returns 502 in some regions)
# apt cache uses a BuildKit cache mount (lists/packages stay in the cache, out of the image layer):
# re-running this layer unpacks from cache instead of downloading debs again
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    set -eux; \
    if [ -n "$APT_MIRROR" ]; then \
        for f in /etc/apt/sources.list.d/debian.sources /etc/apt/sources.list; do \
            if [ -f "$f" ]; then sed -i "s|deb.debian.org|$APT_MIRROR|g; s|security.debian.org|$APT_MIRROR|g" "$f"; fi; \
        done; \
    fi; \
    apt-get update -o Acquire::Retries=3; \
    apt-get install -y --no-install-recommends libgl1 libglib2.0-0 curl build-essential

WORKDIR /app

COPY requirements.txt requirements-torch-cpu.txt requirements-torch-cuda.txt ./
# pip cache also uses a cache mount: re-running this layer installs the big torch wheels from
# cache instead of re-downloading (--no-cache-dir would defeat it, so it is deliberately omitted)
RUN --mount=type=cache,target=/root/.cache/pip \
    set -eux; \
    extra=""; [ -n "$PIP_INDEX" ] && extra="--extra-index-url $PIP_INDEX"; \
    pip install -r requirements.txt $extra; \
    if [ "$VARIANT" = "gpu" ]; then torch_index="https://download.pytorch.org/whl/cu132"; \
    else torch_index="https://download.pytorch.org/whl/cpu"; fi; \
    # PyPI's Linux torch wheel bundles CUDA by default (~5GB), so the CPU variant must use the /cpu index
    [ -n "$TORCH_INDEX" ] && torch_index="$TORCH_INDEX"; \
    if [ "$VARIANT" = "gpu" ]; then \
        pip install -r requirements-torch-cuda.txt --index-url "$torch_index" $extra; \
    else \
        pip install -r requirements-torch-cpu.txt --index-url "$torch_index" $extra; \
    fi

COPY app ./app
COPY bootstrap.py ./
COPY models ./models

# token.txt lives in /data (a compose volume) — container rebuilds keep the token, no client re-paste
RUN mkdir -p /data

EXPOSE 9055

# Health check must send the token, read from the token file (/health is also auth-guarded)
HEALTHCHECK --interval=30s --timeout=5s --start-period=90s --retries=3 \
    CMD curl -fsS -H "x-ezpdf-token: $(cat $EZPDF_TOKEN_FILE)" "http://127.0.0.1:${EZPDF_PORT}/health" || exit 1

CMD ["python", "-m", "app.server_docker"]
