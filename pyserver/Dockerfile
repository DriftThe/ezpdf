# ezpdf 解析服务镜像（CPU / GPU 两种，靠 VARIANT 构建参数切换）
#
#   docker build -t ezpdf-pyserver:cpu --build-arg VARIANT=cpu pyserver
#   docker build -t ezpdf-pyserver:gpu --build-arg VARIANT=gpu pyserver
#
# 模型（models/，~1.9GB）直接 COPY 进镜像 → 自包含快照，容器起来即可推理，不依赖联网。
# torch 的 CUDA 依赖由 pip 的 nvidia-* 轮子自带（宿主只需装好驱动 + nvidia-container-toolkit），
# 因此 GPU 变体也基于 python:3.12-slim，不需要 nvidia/cuda 基础镜像。
FROM python:3.12-slim

ARG VARIANT=cpu
# 国内构建可用镜像源：--build-arg PIP_INDEX=https://pypi.tuna.tsinghua.edu.cn/simple
ARG PIP_INDEX=""

ENV PYTHONUNBUFFERED=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    EZPDF_HOST=0.0.0.0 \
    EZPDF_PORT=9055 \
    EZPDF_MODELS_DIR=/app/models \
    EZPDF_TOKEN_FILE=/data/token.txt \
    EZPDF_MAX_BATCH_PAGES=32

# opencv-contrib-python 需要 libgl/libglib（slim 镜像不自带）
RUN apt-get update \
    && apt-get install -y --no-install-recommends libgl1 libglib2.0-0 curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY requirements.txt requirements-torch-cpu.txt requirements-torch-cuda.txt ./
RUN set -eux; \
    extra=""; [ -n "$PIP_INDEX" ] && extra="--extra-index-url $PIP_INDEX"; \
    pip install --no-cache-dir -r requirements.txt $extra; \
    if [ "$VARIANT" = "gpu" ]; then \
        pip install --no-cache-dir -r requirements-torch-cuda.txt --index-url https://download.pytorch.org/whl/cu132 $extra; \
    else \
        # Linux 的 PyPI torch 轮子默认捆绑 CUDA（~5GB），CPU 变体必须走 /cpu 索引
        pip install --no-cache-dir -r requirements-torch-cpu.txt --index-url https://download.pytorch.org/whl/cpu $extra; \
    fi

COPY app ./app
COPY bootstrap.py ./
COPY models ./models

# token.txt 落在 /data（compose 挂卷）——重建容器不换令牌，客户端不用重新粘贴
RUN mkdir -p /data

EXPOSE 9055

# 健康检查要带 token：从令牌文件读（/health 也在校验范围内）
HEALTHCHECK --interval=30s --timeout=5s --start-period=90s --retries=3 \
    CMD curl -fsS -H "x-ezpdf-token: $(cat $EZPDF_TOKEN_FILE)" "http://127.0.0.1:${EZPDF_PORT}/health" || exit 1

CMD ["python", "-m", "app.server_docker"]
