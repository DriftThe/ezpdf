# ezpdf parse service image (CPU / GPU, switched by the VARIANT build arg)
#   docker build -t ezpdf-pyserver:cpu --build-arg VARIANT=cpu pyserver
# Models (~1.9GB) are COPYed in → self-contained, offline inference-ready. CUDA deps come from pip's
# nvidia-* wheels, so the GPU variant is also python:3.12-slim (no nvidia/cuda base image). The base
# image is swappable via BASE_IMAGE where Docker Hub is unreachable.
ARG BASE_IMAGE=python:3.12-slim
FROM ${BASE_IMAGE}

ARG VARIANT=cpu
# Mirrors for slow/blocked builds (same as the client's "use mirror" switch):
#   APT_MIRROR=mirrors.tuna.tsinghua.edu.cn  PIP_INDEX=https://pypi.tuna.tsinghua.edu.cn/simple
#   TORCH_INDEX=https://mirror.sjtu.edu.cn/pytorch-wheels/cpu   # GPU variant: .../cu132
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

# libgl/libglib for opencv-contrib-python; build-essential because the CUDA path JIT-compiles a triton
# kernel at runtime (needs gcc/g++ + libc6 headers). Packages are deliberately not trimmed for size.
# apt cache lives in a BuildKit cache mount, so re-running this layer unpacks from cache.
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
